use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CONFIG_FILE_NAME: &str = "booster_config.json";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    pub gain_db: f32,
    pub input_device: String,
    pub output_device: String,
    #[serde(default)]
    pub silent_start: bool,
    #[serde(default)]
    pub auto_start: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            gain_db: 14.0,
            input_device: String::new(),
            output_device: String::new(),
            silent_start: false,
            auto_start: false,
        }
    }
}

impl AppConfig {
    fn config_path() -> PathBuf {
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(parent) = exe_path.parent() {
                return parent.join(CONFIG_FILE_NAME);
            }
        }
        PathBuf::from(CONFIG_FILE_NAME)
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<AppConfig>(&content) {
                    return config;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Ok(content) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&path, content);
        }
    }
}
