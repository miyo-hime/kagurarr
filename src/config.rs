use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub lidarr: LidarrConfig,
    pub slskd: SlskdConfig,
    pub kagurarr: KagurConfig,
    pub database: DatabaseConfig,
}

#[derive(Debug, Deserialize)]
pub struct LidarrConfig {
    pub url: String,
    pub api_key: String,
    // lidarr sees the download dir at a different path than slskd does (different container mounts).
    // if unset, falls back to slskd.download_dir - which works if both containers share the same mount path.
    #[serde(default)]
    pub download_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SlskdConfig {
    pub url: String,
    pub api_key: String,
    pub download_dir: String,
}

#[derive(Debug, Deserialize)]
pub struct KagurConfig {
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_min_score")]
    pub min_score: f64,
    #[serde(default = "default_formats")]
    pub preferred_formats: Vec<String>,
    #[serde(default = "default_max_albums")]
    pub max_albums_per_run: usize,
    #[serde(default = "default_blacklist_ttl_days")]
    pub blacklist_ttl_days: u64,
    #[serde(default = "default_stall_timeout")]
    pub stall_timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    #[serde(default = "default_db_path")]
    pub path: String,
}

// defaults - a minimal config only needs [lidarr] and [slskd]
fn default_interval() -> u64 { 600 }
fn default_min_score() -> f64 { 0.75 } // don't go lower. you will regret it.
fn default_formats() -> Vec<String> { vec!["flac".into(), "mp3".into()] }
fn default_max_albums() -> usize { 10 }
fn default_blacklist_ttl_days() -> u64 { 30 }
fn default_stall_timeout() -> u64 { 300 }
fn default_db_path() -> String { "/data/kagurarr.db".into() }

pub fn load(path: &str) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("couldn't read config at {path}"))?;
    toml::from_str(&raw).with_context(|| format!("couldn't parse config at {path}"))
}
