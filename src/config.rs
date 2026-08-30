use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::params::ParamState;
use crate::theme::ThemeMode;

const APP_NAME: &str = "llamacpp-manager";
pub const DEFAULT_PARAMS_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/tsybindev/llamacpp-manager/main/assets/params_catalog.json";

/// Automatic restart policy for a crashed llama-server process.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct AutoRestore {
    pub enabled: bool,
    pub max_restarts: u32,
    /// Time window in seconds; restarts older than this are forgotten.
    pub window_secs: u64,
    /// First backoff delay in seconds, doubles after each attempt.
    pub backoff_start_secs: u64,
}

impl Default for AutoRestore {
    fn default() -> Self {
        Self {
            enabled: true,
            max_restarts: 3,
            window_secs: 5 * 60,
            backoff_start_secs: 2,
        }
    }
}

/// Application settings persisted as JSON in the user config directory.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub models_dir: PathBuf,
    pub builds_dir: PathBuf,
    pub logs_dir: PathBuf,
    /// HuggingFace access token, stored in plain text (the user is warned in the UI).
    pub hf_token: String,
    pub theme: ThemeMode,
    pub sidebar_collapsed: bool,
    /// UI zoom factor (0.8–1.5).
    pub ui_zoom: f32,
    /// When true, the logger also records debug-level messages.
    pub debug_logging: bool,
    pub auto_restore: AutoRestore,
    pub params_catalog_url: String,
    /// Last used parameter values (persisted between runs; presets come later).
    pub params: ParamState,
    /// Preset selected in the previous session, restored at startup.
    pub last_preset: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let data_dir = project_dir()
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(default_fallback_dir);
        Self {
            models_dir: data_dir.join("models"),
            builds_dir: data_dir.join("builds"),
            logs_dir: data_dir.join("logs"),
            hf_token: String::new(),
            theme: ThemeMode::System,
            sidebar_collapsed: false,
            ui_zoom: 1.0,
            debug_logging: false,
            auto_restore: AutoRestore::default(),
            params_catalog_url: DEFAULT_PARAMS_CATALOG_URL.to_string(),
            params: ParamState::default(),
            last_preset: None,
        }
    }
}

impl Settings {
    /// Create the data directories referenced by the settings.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [&self.models_dir, &self.builds_dir, &self.logs_dir] {
            fs::create_dir_all(dir)
                .with_context(|| format!("не удалось создать каталог {}", dir.display()))?;
        }
        Ok(())
    }
}

fn project_dir() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", APP_NAME)
}

fn default_fallback_dir() -> PathBuf {
    std::env::temp_dir().join(APP_NAME)
}

pub fn config_path() -> PathBuf {
    let config_dir = project_dir()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(default_fallback_dir);
    config_dir.join("settings.json")
}

/// Load settings from the default config path; missing or broken files fall
/// back to defaults (the caller is expected to log the reason).
pub fn load() -> (Settings, Option<String>) {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> (Settings, Option<String>) {
    match fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(settings) => (settings, None),
            Err(e) => (Settings::default(), Some(format!("файл повреждён, применены значения по умолчанию: {e}"))),
        },
        Err(_) => (Settings::default(), None),
    }
}

/// Save settings atomically: write to a temp file next to the target, then rename.
pub fn save(settings: &Settings) -> Result<()> {
    save_to(settings, &config_path())
}

pub fn save_to(settings: &Settings, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("не удалось создать каталог {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(settings).context("не удалось сериализовать настройки")?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).with_context(|| format!("не удалось записать {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("не удалось обновить {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_all_fields() {
        let dir = std::env::temp_dir().join(format!("llamacpp-manager-test-{}", std::process::id()));
        let path = dir.join("settings.json");
        let settings = Settings {
            hf_token: "hf_test_token".into(),
            debug_logging: true,
            theme: ThemeMode::Dark,
            models_dir: dir.join("models"),
            ..Settings::default()
        };
        save_to(&settings, &path).expect("save");
        let (loaded, warning) = load_from(&path);
        assert!(warning.is_none());
        assert_eq!(loaded, settings);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn broken_file_falls_back_to_defaults() {
        let dir = std::env::temp_dir().join(format!("llamacpp-manager-test-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let (settings, warning) = load_from(&path);
        assert!(warning.is_some());
        assert_eq!(settings, Settings::default());
        std::fs::remove_dir_all(&dir).ok();
    }
}
