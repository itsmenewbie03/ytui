use serde::{Deserialize, Serialize};
use std::{env, fs, io::Write, path::PathBuf};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

const DEFAULT_ACCENT: &str = "Sky";

#[derive(Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub accent: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accent: DEFAULT_ACCENT.to_owned(),
        }
    }
}

impl Config {
    pub fn load() -> Result<Self, String> {
        let path = config_path()?;
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("could not read {}: {error}", path.display())),
        };
        toml::from_str(&contents)
            .map_err(|error| format!("could not parse {}: {error}", path.display()))
    }

    pub fn save(&self) -> Result<(), String> {
        let path = config_path()?;
        let directory = path
            .parent()
            .ok_or_else(|| "configuration path has no parent directory".to_owned())?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let contents = toml::to_string_pretty(self)
            .map_err(|error| format!("could not serialize configuration: {error}"))?;
        let temporary_path = path.with_extension("toml.tmp");
        fs::write(&temporary_path, contents)
            .map_err(|error| format!("could not write {}: {error}", temporary_path.display()))?;
        fs::rename(&temporary_path, &path)
            .map_err(|error| format!("could not replace {}: {error}", path.display()))
    }
}

pub struct Credentials {
    cookie: String,
}

impl Credentials {
    pub fn load() -> Result<Option<Self>, String> {
        let path = credentials_path()?;
        let cookie = match fs::read_to_string(&path) {
            Ok(cookie) => cookie,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("could not read {}: {error}", path.display())),
        };
        normalize_cookie(&cookie).map(|cookie| Some(Self { cookie }))
    }

    pub fn new(cookie: &str) -> Result<Self, String> {
        normalize_cookie(cookie).map(|cookie| Self { cookie })
    }

    pub fn cookie(&self) -> &str {
        &self.cookie
    }

    pub fn save(&self) -> Result<(), String> {
        let path = credentials_path()?;
        let directory = path
            .parent()
            .ok_or_else(|| "credentials path has no parent directory".to_owned())?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
        let temporary_path = path.with_extension("tmp");
        write_private_file(&temporary_path, self.cookie.as_bytes())?;
        fs::rename(&temporary_path, &path)
            .map_err(|error| format!("could not replace {}: {error}", path.display()))?;
        set_private_permissions(&path)
    }

    pub fn remove() -> Result<(), String> {
        let path = credentials_path()?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("could not remove {}: {error}", path.display())),
        }
    }
}

fn normalize_cookie(input: &str) -> Result<String, String> {
    if input.contains(['\r', '\n']) {
        return Err("cookie must be a single request-header line".to_owned());
    }
    let mut cookie = input.trim();
    if cookie
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cookie:"))
    {
        cookie = cookie[7..].trim();
    }
    if cookie.is_empty() {
        return Err("cookie cannot be empty".to_owned());
    }
    if cookie.len() > 16 * 1024 {
        return Err("cookie exceeds the 16 KiB limit".to_owned());
    }
    let has_sapisid = cookie.split(';').any(|part| {
        part.split_once('=')
            .is_some_and(|(name, value)| name.trim() == "SAPISID" && !value.trim().is_empty())
    });
    if !has_sapisid {
        return Err("cookie is missing SAPISID".to_owned());
    }
    Ok(cookie.to_owned())
}

fn credentials_path() -> Result<PathBuf, String> {
    Ok(config_directory()?.join("credentials"))
}

fn write_private_file(path: &PathBuf, contents: &[u8]) -> Result<(), String> {
    let mut options = fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    file.write_all(contents)
        .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    set_private_permissions(path)
}

fn set_private_permissions(path: &PathBuf) -> Result<(), String> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("could not secure {}: {error}", path.display()))?;
    Ok(())
}

fn config_path() -> Result<PathBuf, String> {
    Ok(config_directory()?.join("config.toml"))
}

fn config_directory() -> Result<PathBuf, String> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| "neither XDG_CONFIG_HOME nor HOME is available".to_owned())?;
    Ok(base.join("ytui"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let config = toml::from_str::<Config>("").expect("empty config should use defaults");
        assert_eq!(config.accent, DEFAULT_ACCENT);
    }

    #[test]
    fn normalizes_cookie_header() {
        let cookie = normalize_cookie(" Cookie: SID=abc; SAPISID=secret ")
            .expect("valid cookie should normalize");
        assert_eq!(cookie, "SID=abc; SAPISID=secret");
    }

    #[test]
    fn rejects_cookie_without_sapisid() {
        assert_eq!(
            normalize_cookie("SID=abc").unwrap_err(),
            "cookie is missing SAPISID"
        );
    }
}
