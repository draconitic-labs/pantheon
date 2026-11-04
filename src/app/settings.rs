//! The few preferences worth remembering between runs. A small JSON file in the
//! user's config directory. Missing or unreadable means defaults, never an error.

use std::path::PathBuf;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// the first-run hint has been dismissed
    pub intro_seen: bool,
    /// ambience stays off until asked for: nothing should make a sound by surprise
    pub ambience_on: bool,
    pub ambience_vol: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            intro_seen: false,
            ambience_on: false,
            ambience_vol: 0.5,
        }
    }
}

fn config_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))?;
    Some(dir.join("pantheon").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        config_path()
            .and_then(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    fn load_from(path: &std::path::Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) {
        if let Some(p) = config_path() {
            if let Err(e) = self.save_to(&p) {
                log::warn!("could not save settings to {}: {e}", p.display());
            }
        }
    }

    fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_bad_or_partial_files() {
        let dir = std::env::temp_dir().join(format!("pantheon-settings-{}", std::process::id()));
        let path = dir.join("nested").join("settings.json");
        let s = Settings {
            intro_seen: true,
            ambience_on: true,
            ambience_vol: 0.25,
        };
        s.save_to(&path).unwrap();
        let back = Settings::load_from(&path).unwrap();
        assert!(back.intro_seen && back.ambience_on);
        assert!((back.ambience_vol - 0.25).abs() < 1e-6);
        // an older file with fewer fields keeps defaults for the rest
        std::fs::write(&path, r#"{"intro_seen": true}"#).unwrap();
        let partial = Settings::load_from(&path).unwrap();
        assert!(partial.intro_seen && !partial.ambience_on);
        // garbage is not an error
        std::fs::write(&path, "not json").unwrap();
        assert!(Settings::load_from(&path).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ambience_defaults_to_silent() {
        assert!(!Settings::default().ambience_on);
    }
}
