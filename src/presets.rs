use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::params::ParamState;

/// A named launch configuration: server form fields + parameter state.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub binary: PathBuf,
    pub model: PathBuf,
    pub host: String,
    pub port: u16,
    /// Raw extra-arguments line (shell-style, parsed with shlex).
    #[serde(default)]
    pub extra_args: String,
    #[serde(default)]
    pub params: ParamState,
}

/// Presets are stored one JSON file per preset in the presets directory.
pub struct PresetStore {
    dir: PathBuf,
}

impl PresetStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Names of stored presets, sorted alphabetically.
    pub fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .map(|entries| {
                entries
                    .filter_map(std::result::Result::ok)
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                    .filter_map(|e| e.path().file_stem()?.to_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    fn path_for(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.json", sanitize_name(name)))
    }

    pub fn save(&self, preset: &Preset) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("не удалось создать каталог {}", self.dir.display()))?;
        let path = self.path_for(&preset.name);
        let json = serde_json::to_string_pretty(preset)
            .context("не удалось сериализовать пресет")?;
        // Atomic write so an interrupted save cannot corrupt an existing preset.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).with_context(|| format!("не удалось записать {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("не удалось обновить {}", path.display()))?;
        Ok(())
    }

    pub fn load(&self, name: &str) -> Result<Preset> {
        let path = self.path_for(name);
        let text = fs::read_to_string(&path)
            .with_context(|| format!("не удалось прочитать {}", path.display()))?;
        let mut preset: Preset =
            serde_json::from_str(&text).with_context(|| format!("пресет «{name}» повреждён"))?;
        preset.name = name.to_string();
        Ok(preset)
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let path = self.path_for(name);
        fs::remove_file(&path).with_context(|| format!("не удалось удалить {}", path.display()))
    }
}

/// Write a preset to an arbitrary file (the user picks the location).
pub fn export_to(preset: &Preset, path: &Path) -> Result<()> {
    let json = serde_json::to_string_pretty(preset).context("не удалось сериализовать пресет")?;
    fs::write(path, json).with_context(|| format!("не удалось записать {}", path.display()))
}

/// Read a preset from an arbitrary file; missing `name` is filled from the file stem.
pub fn import_from(path: &Path) -> Result<Preset> {
    let text =
        fs::read_to_string(path).with_context(|| format!("не удалось прочитать {}", path.display()))?;
    let mut preset: Preset = serde_json::from_str(&text)
        .with_context(|| format!("файл {} не является пресетом", path.display()))?;
    if preset.name.is_empty() {
        preset.name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("импортирован")
            .to_string();
    }
    Ok(preset)
}

/// Map an arbitrary preset name to a safe file name component.
/// Spaces and common separators are kept readable, everything exotic is replaced.
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ') {
                if c == ' ' { '_' } else { c }
            } else {
                '_'
            }
        })
        .collect();
    // Trim separator padding and leading/trailing dots so the result can
    // never form a "." / ".." path component.
    let trimmed = cleaned
        .trim_matches(|c| c == '_' || c == '.')
        .to_string();
    if trimmed.is_empty() {
        "preset".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> Preset {
        let mut params = ParamState::default();
        params.set("context-size", true);
        params.set_value("context-size", serde_json::json!(65536));
        params.set("jinja", true);
        Preset {
            name: name.to_string(),
            binary: PathBuf::from("/opt/llama/llama-server"),
            model: PathBuf::from("/models/gemma.gguf"),
            host: "0.0.0.0".into(),
            port: 8080,
            extra_args: "--verbose".into(),
            params,
        }
    }

    fn temp_store(tag: &str) -> (PresetStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "llamacpp-manager-presets-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (PresetStore::new(dir.clone()), dir)
    }

    #[test]
    fn save_load_roundtrip_preserves_everything() {
        let (store, dir) = temp_store("roundtrip");
        let preset = sample("Мой пресет 2");
        store.save(&preset).expect("save");
        let loaded = store.load("Мой пресет 2").expect("load");
        assert_eq!(loaded, preset);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_returns_sorted_names() {
        let (store, dir) = temp_store("list");
        store.save(&sample("b")).unwrap();
        store.save(&sample("a")).unwrap();
        store.save(&sample("c")).unwrap();
        assert_eq!(store.list(), vec!["a", "b", "c"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_removes_file_and_missing_load_fails() {
        let (store, dir) = temp_store("delete");
        store.save(&sample("tmp")).unwrap();
        store.delete("tmp").unwrap();
        assert!(store.load("tmp").is_err());
        assert!(store.delete("tmp").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn export_import_roundtrip() {
        let dir = std::env::temp_dir().join(format!("llamacpp-manager-presets-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let preset = sample("exported");
        let path = dir.join("shared.json");
        export_to(&preset, &path).unwrap();
        let mut imported = import_from(&path).unwrap();
        assert_eq!(imported, preset);
        // Name-less files take the file stem as the preset name.
        imported.name = String::new();
        export_to(&imported, &path).unwrap();
        assert_eq!(import_from(&path).unwrap().name, "shared");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sanitize_maps_names_to_safe_file_names() {
        assert_eq!(sanitize_name("my cool preset"), "my_cool_preset");
        assert_eq!(sanitize_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_name("  "), "preset");
        assert_eq!(sanitize_name("../etc/passwd"), "etc_passwd");
    }
}
