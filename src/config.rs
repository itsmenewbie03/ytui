use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};

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

fn config_path() -> Result<PathBuf, String> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .ok_or_else(|| "neither XDG_CONFIG_HOME nor HOME is available".to_owned())?;
    Ok(base.join("ytui").join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_use_defaults() {
        let config = toml::from_str::<Config>("").expect("empty config should use defaults");
        assert_eq!(config.accent, DEFAULT_ACCENT);
    }
}
