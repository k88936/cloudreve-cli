use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub default: ServerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServerConfig {
    pub server: Option<String>,
    pub email: Option<String>,
    pub password: Option<String>,
}

impl ServerConfig {
    pub fn resolve(&self) -> ResolvedConfig {
        ResolvedConfig {
            server: env::var("CLOUDREVE_SERVER")
                .ok()
                .or_else(|| self.server.clone())
                .expect("CLOUDREVE_SERVER not set and no server in config"),
            email: env::var("CLOUDREVE_EMAIL")
                .ok()
                .or_else(|| self.email.clone())
                .expect("CLOUDREVE_EMAIL not set and no email in config"),
            password: env::var("CLOUDREVE_PASSWORD")
                .ok()
                .or_else(|| self.password.clone())
                .expect("CLOUDREVE_PASSWORD not set and no password in config"),
        }
    }

    pub fn is_configured(&self) -> bool {
        let has_server = env::var("CLOUDREVE_SERVER").is_ok() || self.server.is_some();
        let has_email = env::var("CLOUDREVE_EMAIL").is_ok() || self.email.is_some();
        let has_password = env::var("CLOUDREVE_PASSWORD").is_ok() || self.password.is_some();
        has_server && has_email && has_password
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub server: String,
    pub email: String,
    pub password: String,
}

fn config_path() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join("cloudreve-cli.toml")
}

pub fn load() -> Result<Config> {
    let path = config_path();
    if path.exists() {
        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        let config: Config =
            toml::from_str(&content).with_context(|| "Failed to parse config file")?;
        Ok(config)
    } else {
        Ok(Config::default())
    }
}

pub fn save(config: &Config) -> Result<()> {
    let path = config_path();
    let content = toml::to_string_pretty(config).with_context(|| "Failed to serialize config")?;
    fs::write(&path, content)
        .with_context(|| format!("Failed to write config file: {}", path.display()))?;
    Ok(())
}

pub fn save_credentials(server: &str, email: &str, password: &str) -> Result<()> {
    let config = Config {
        default: ServerConfig {
            server: Some(server.to_string()),
            email: Some(email.to_string()),
            password: Some(password.to_string()),
        },
    };
    save(&config)
}
