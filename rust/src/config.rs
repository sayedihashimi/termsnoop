use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Application configuration, loaded from `~/.termsnoop/config.toml`.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    /// Maximum age of sessions before `clean` removes them (days).
    pub session_ttl_days: u64,

    /// Maximum log file size per session (bytes).
    pub max_log_bytes: u64,

    /// Default shell to spawn.
    pub default_shell: Option<String>,

    /// Number of commands to keep in shell history (default 500).
    pub command_history_size: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            session_ttl_days: 7,
            max_log_bytes: 50 * 1024 * 1024,
            default_shell: None,
            command_history_size: 500,
        }
    }
}

impl Config {
    /// Load config from `~/.termsnoop/config.toml`, falling back to defaults.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(cfg)
    }

    /// Path to the config file.
    pub fn path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        Ok(home.join(".termsnoop").join("config.toml"))
    }
}
