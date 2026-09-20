use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

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

    pub fn from_netscape_file(path: &str) -> Result<Self, String> {
        let path = expand_home(path)?;
        let contents = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        parse_netscape_cookies(&contents).map(|cookie| Self { cookie })
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

fn parse_netscape_cookies(input: &str) -> Result<String, String> {
    let header = input
        .lines()
        .next()
        .map(|line| line.trim_start_matches('\u{feff}'));
    if !matches!(
        header,
        Some("# HTTP Cookie File" | "# Netscape HTTP Cookie File")
    ) {
        return Err("file is not a Netscape cookies.txt export".to_owned());
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before the Unix epoch: {error}"))?
        .as_secs();
    let mut cookies = Vec::new();

    for (index, line) in input.lines().enumerate().skip(1) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || (line.starts_with('#') && !line.starts_with("#HttpOnly_")) {
            continue;
        }
        let fields = line.splitn(7, '\t').collect::<Vec<_>>();
        if fields.len() != 7 {
            return Err(format!(
                "invalid Netscape cookie record on line {}",
                index + 1
            ));
        }

        let domain = fields[0]
            .strip_prefix("#HttpOnly_")
            .unwrap_or(fields[0])
            .trim_start_matches('.')
            .to_ascii_lowercase();
        if domain != "music.youtube.com" && !"music.youtube.com".ends_with(&format!(".{domain}")) {
            continue;
        }
        if fields[2] != "/" && !"/youtubei/v1/browse".starts_with(fields[2]) {
            continue;
        }
        let expires = fields[4]
            .parse::<u64>()
            .map_err(|_| format!("invalid cookie expiration on line {}", index + 1))?;
        if expires != 0 && expires <= now {
            continue;
        }
        if fields[5].is_empty() {
            return Err(format!("cookie name is empty on line {}", index + 1));
        }
        cookies.push(format!("{}={}", fields[5], fields[6]));
    }

    normalize_cookie(&cookies.join("; "))
}

fn expand_home(input: &str) -> Result<PathBuf, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("cookie file path cannot be empty".to_owned());
    }
    if input == "~" {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not available for ~ expansion".to_owned());
    }
    if let Some(relative) = input.strip_prefix("~/") {
        return env::var_os("HOME")
            .map(|home| Path::new(&home).join(relative))
            .ok_or_else(|| "HOME is not available for ~ expansion".to_owned());
    }
    Ok(PathBuf::from(input))
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

    #[test]
    fn converts_netscape_youtube_cookies_to_header() {
        let input = concat!(
            "# Netscape HTTP Cookie File\n",
            ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc\n",
            "#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsecret\n",
            ".google.com\tTRUE\t/\tTRUE\t0\tNID\tignored\n",
            "www.youtube.com\tFALSE\t/\tTRUE\t0\tOTHER\tignored\n",
        );

        let cookie = parse_netscape_cookies(input).expect("valid export should parse");

        assert_eq!(cookie, "SID=abc; SAPISID=secret");
    }

    #[test]
    fn rejects_non_netscape_cookie_file() {
        assert_eq!(
            parse_netscape_cookies("SAPISID=secret").unwrap_err(),
            "file is not a Netscape cookies.txt export"
        );
    }

    #[test]
    fn rejects_netscape_export_without_applicable_sapisid() {
        let input = concat!(
            "# HTTP Cookie File\n",
            ".youtube.com\tTRUE\t/\tTRUE\t0\tSID\tabc\n",
            "www.youtube.com\tFALSE\t/\tTRUE\t0\tSAPISID\tsecret\n",
        );

        assert_eq!(
            parse_netscape_cookies(input).unwrap_err(),
            "cookie is missing SAPISID"
        );
    }
}
