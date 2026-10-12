use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const CONFIG_FILE_NAME: &str = "booster_config.json";

fn default_gain() -> f32 {
    14.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AppConfig {
    #[serde(default = "default_gain")]
    pub gain_db: f32,
    pub input_device: String,
    pub output_device: String,
    #[serde(default)]
    pub silent_start: bool,
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub device_gains: HashMap<String, f32>,
    #[serde(default)]
    pub device_history: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            gain_db: 14.0,
            input_device: String::new(),
            output_device: String::new(),
            silent_start: false,
            auto_start: false,
            device_gains: HashMap::new(),
            device_history: Vec::new(),
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
                if let Ok(mut config) = serde_json::from_str::<AppConfig>(&content) {
                    // 如果存在旧版配置且尚未记录当前输入设备增益，自动迁移补充
                    if !config.input_device.is_empty() && !config.device_gains.contains_key(&config.input_device) {
                        config.device_gains.insert(config.input_device.clone(), config.gain_db);
                    }
                    if !config.input_device.is_empty() && !config.device_history.contains(&config.input_device) {
                        config.device_history.push(config.input_device.clone());
                    }
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

    pub fn get_gain_for_device(&self, device_name: &str) -> f32 {
        if device_name.is_empty() {
            return self.gain_db;
        }
        self.device_gains.get(device_name).copied().unwrap_or(self.gain_db)
    }

    pub fn set_gain_for_device(&mut self, device_name: &str, gain: f32) {
        if !device_name.is_empty() {
            self.device_gains.insert(device_name.to_string(), gain);
        }
        self.gain_db = gain;
    }

    pub fn push_device_history(&mut self, device: &str) {
        if device.is_empty() {
            return;
        }
        self.device_history.retain(|d| d != device);
        self.device_history.push(device.to_string());
    }

    pub fn get_fallback_device(&self, current_devices: &[String]) -> Option<String> {
        // 从历史记录的末尾（最近使用过的）往前寻找，找到第一个仍然在线的设备
        for dev in self.device_history.iter().rev() {
            if current_devices.contains(dev) {
                return Some(dev.clone());
            }
        }
        current_devices.first().cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_per_device_gains() {
        let mut config = AppConfig::default();
        config.set_gain_for_device("Mic A", 22.0);
        config.set_gain_for_device("Mic B", 14.0);
        config.set_gain_for_device("Mic C", 6.0);

        assert_eq!(config.get_gain_for_device("Mic A"), 22.0);
        assert_eq!(config.get_gain_for_device("Mic B"), 14.0);
        assert_eq!(config.get_gain_for_device("Mic C"), 6.0);
        // 未记录的设备使用默认 gain_db
        assert_eq!(config.get_gain_for_device("Mic Unknown"), 6.0);
    }

    #[test]
    fn test_legacy_json_migration() {
        let json_str = r#"{
            "gain_db": 18.0,
            "input_device": "Old USB Mic",
            "output_device": "CABLE Input"
        }"#;

        let mut config: AppConfig = serde_json::from_str(json_str).unwrap();
        assert!(config.device_gains.is_empty());
        // 模拟 load() 中的自动迁移逻辑
        if !config.input_device.is_empty() && !config.device_gains.contains_key(&config.input_device) {
            config.device_gains.insert(config.input_device.clone(), config.gain_db);
        }

        assert_eq!(config.get_gain_for_device("Old USB Mic"), 18.0);
    }

    #[test]
    fn test_json_roundtrip_with_device_gains() {
        let mut config = AppConfig::default();
        config.input_device = "Mic A".to_string();
        config.set_gain_for_device("Mic A", 16.5);
        config.set_gain_for_device("Mic B", 24.0);

        let json_str = serde_json::to_string(&config).unwrap();
        let loaded: AppConfig = serde_json::from_str(&json_str).unwrap();

        assert_eq!(loaded.get_gain_for_device("Mic A"), 16.5);
        assert_eq!(loaded.get_gain_for_device("Mic B"), 24.0);
    }

    #[test]
    fn test_device_fallback_history() {
        let mut config = AppConfig::default();
        config.push_device_history("Redmi Speaker Mic");
        config.push_device_history("Wireless Mic Rx");
        config.push_device_history("DJI Mic Mini");

        // 1. DJI Mic Mini 拔出，在线设备为 Redmi 与 Wireless Mic -> 回退到 Wireless Mic Rx
        let current_online = vec!["Redmi Speaker Mic".to_string(), "Wireless Mic Rx".to_string()];
        assert_eq!(
            config.get_fallback_device(&current_online),
            Some("Wireless Mic Rx".to_string())
        );

        // 2. Wireless Mic Rx 也拔出，仅剩 Redmi -> 回退到 Redmi Speaker Mic
        let only_redmi = vec!["Redmi Speaker Mic".to_string()];
        assert_eq!(
            config.get_fallback_device(&only_redmi),
            Some("Redmi Speaker Mic".to_string())
        );

        // 3. 重新插入 DJI Mic Mini -> 重新推入历史记录顶部
        config.push_device_history("DJI Mic Mini");
        let redmi_and_dji = vec!["Redmi Speaker Mic".to_string(), "DJI Mic Mini".to_string()];
        assert_eq!(
            config.get_fallback_device(&redmi_and_dji),
            Some("DJI Mic Mini".to_string())
        );

        // 4. 全部拔出 -> None
        let none_online: Vec<String> = vec![];
        assert_eq!(config.get_fallback_device(&none_online), None);

        // 5. 在线设备不在历史记录中（例如全新插入的未记录设备） -> 回退到该首个可用在线设备
        let unknown_device = vec!["Brand New Studio Mic".to_string()];
        assert_eq!(
            config.get_fallback_device(&unknown_device),
            Some("Brand New Studio Mic".to_string())
        );
    }
}
