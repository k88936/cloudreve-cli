use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CONFIG_DIR: &str = "cloudreve";
const CONFIG_FILE: &str = "cli.toml";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub active_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub user_email: Option<String>,
    #[serde(default)]
    pub user_nickname: Option<String>,
}

impl Config {
    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(CONFIG_DIR)
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join(CONFIG_FILE)
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config from {:?}", path))?;
        toml::from_str(&content).with_context(|| format!("Failed to parse config from {:?}", path))
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        if !dir.exists() {
            fs::create_dir_all(&dir)
                .with_context(|| format!("Failed to create config directory {:?}", dir))?;
        }
        let path = Self::config_path();
        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write config to {:?}", path))?;
        Ok(())
    }

    pub fn get_active_profile(&self) -> Option<&Profile> {
        let active_name = self.active_profile.as_ref()?;
        self.profiles.iter().find(|p| &p.name == active_name)
    }

    pub fn add_or_update_profile(&mut self, profile: Profile) {
        if let Some(existing) = self.profiles.iter_mut().find(|p| p.name == profile.name) {
            *existing = profile;
        } else {
            self.profiles.push(profile);
        }
    }

    pub fn remove_profile(&mut self, name: &str) -> bool {
        let len_before = self.profiles.len();
        self.profiles.retain(|p| p.name != name);
        if self.active_profile.as_deref() == Some(name) {
            self.active_profile = self.profiles.first().map(|p| p.name.clone());
        }
        self.profiles.len() < len_before
    }
}

pub fn env_config() -> Option<Profile> {
    let server = std::env::var("CLOUDREVE_SERVER").ok()?;
    let access_token = std::env::var("CLOUDREVE_ACCESS_TOKEN").ok()?;
    let refresh_token =
        std::env::var("CLOUDREVE_REFRESH_TOKEN").unwrap_or_else(|_| access_token.clone());

    Some(Profile {
        name: "env".to_string(),
        server,
        access_token,
        refresh_token,
        user_email: std::env::var("CLOUDREVE_EMAIL").ok(),
        user_nickname: None,
    })
}
