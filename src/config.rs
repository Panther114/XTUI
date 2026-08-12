use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Config {
    pub source: Option<String>,
    pub client_id: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub user_id: Option<String>,
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

    pub fn use_browser(&self) -> bool {
        self.source.as_deref() == Some("browser")
    }
}
