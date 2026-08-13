use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};
use std::{collections::HashMap, fs, path::PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    pub source: Option<String>,
    pub client_id: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub user_id: Option<String>,
    /// Seconds between silent background refreshes of the Home timeline.
    /// 0 disables auto-refresh.
    #[serde(default)]
    pub auto_refresh_secs: Option<u64>,
    #[serde(default)]
    pub theme: Option<ThemeConfig>,
    /// Action name → key spec(s). Accepts either a string or an array of
    /// strings: `{"move_down": "j"}` or `{"back": ["left", "esc"]}`.
    #[serde(default)]
    pub keys: Option<KeyConfig>,
}

/// Hex intensity overrides for XTUI's monochrome palette. Values are converted
/// to grayscale when loaded, so customization cannot introduce hue.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ThemeConfig {
    pub accent: Option<String>,
    pub white: Option<String>,
    pub gray: Option<String>,
    pub dim: Option<String>,
    pub surface: Option<String>,
    pub surface_raised: Option<String>,
    pub green: Option<String>,
    pub amber: Option<String>,
    pub red: Option<String>,
    pub background: Option<String>,
}

/// A JSON object of action → key binding(s), with per-value flexibility.
#[derive(Clone, Debug, Default)]
pub struct KeyConfig(pub HashMap<String, Vec<String>>);

impl Serialize for KeyConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, keys) in &self.0 {
            map.serialize_entry(name, keys)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for KeyConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany {
            One(String),
            Many(Vec<String>),
        }
        let raw = HashMap::<String, OneOrMany>::deserialize(deserializer)?;
        Ok(KeyConfig(
            raw.into_iter()
                .map(|(name, value)| {
                    (
                        name,
                        match value {
                            OneOrMany::One(one) => vec![one],
                            OneOrMany::Many(many) => many,
                        },
                    )
                })
                .collect(),
        ))
    }
}

impl Config {
    pub fn path() -> Result<PathBuf> {
        let root = dirs::config_dir().context("your OS did not provide a config directory")?;
        Ok(root.join("xtui").join("config.json"))
    }

    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        let mut config = if path.exists() {
            serde_json::from_slice(&fs::read(&path).context("could not read XTUI config")?)
                .context("XTUI config is not valid JSON")?
        } else {
            Self::default()
        };
        if let Ok(value) = std::env::var("XTUI_CLIENT_ID") {
            config.client_id = Some(value);
        }
        if let Ok(value) = std::env::var("XTUI_ACCESS_TOKEN") {
            config.access_token = Some(value);
        }
        Ok(config)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("could not write {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref().filter(|s| !s.is_empty())
    }

    pub fn use_extension(&self) -> bool {
        matches!(self.source.as_deref(), Some("extension" | "browser"))
    }

    /// Mark the browser extension as the preferred source and persist it so
    /// plain `xtui` launches resolve to browser mode.
    pub fn enable_extension_mode() -> Result<()> {
        let mut config = Config::load()?;
        config.source = Some("extension".into());
        config.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_accepts_single_or_array_values() {
        let json = r#"{"move_down":"s","back":["left","esc"]}"#;
        let config: KeyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.0["move_down"], vec!["s"]);
        assert_eq!(config.0["back"], vec!["left", "esc"]);
    }

    #[test]
    fn full_config_round_trips_with_new_fields() {
        let json = r##"{
            "source": "extension",
            "auto_refresh_secs": 120,
            "theme": {"accent": "#FF0000"},
            "keys": {"quit": ["ctrl-c"]}
        }"##;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.auto_refresh_secs, Some(120));
        assert_eq!(config.theme.unwrap().accent.as_deref(), Some("#FF0000"));
        assert_eq!(config.keys.unwrap().0["quit"], vec!["ctrl-c"]);
        let minimal: Config = serde_json::from_str("{}").unwrap();
        assert!(minimal.auto_refresh_secs.is_none());
        assert!(minimal.keys.is_none());
        assert!(minimal.theme.is_none());
    }
}
