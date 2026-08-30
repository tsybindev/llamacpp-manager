use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Widget/logic type of a parameter.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ParamKind {
    Bool,
    Int,
    Float,
    Enum,
    String,
    Path,
}

/// A single parameter definition from the catalog.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ParamDef {
    pub id: String,
    pub flag: String,
    #[serde(default)]
    pub short: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub category: String,
    pub kind: ParamKind,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub options: Vec<String>,
    /// Whether the parameter starts enabled. Bool parameters with a llama.cpp
    /// default of "enabled" are passed explicitly; others start off.
    #[serde(default)]
    pub enabled_default: bool,
}

/// The parameter catalog: bundled, cached and/or remotely updated.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ParamsCatalog {
    pub version: u32,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub source: String,
    pub params: Vec<ParamDef>,
}

impl ParamsCatalog {
    pub fn categories(&self) -> Vec<(&'static str, &'static str)> {
        // Stable order of known categories; unknown ones are appended by
        // first appearance in the catalog.
        let mut known: Vec<(&'static str, &'static str)> = vec![
            ("context", "Модель и контекст"),
            ("gpu", "GPU и разгрузка"),
            ("kv", "Память и KV-кэш"),
            ("spec", "Спекулятивный декодинг (draft)"),
            ("sampling", "Сэмплинг"),
            ("server", "HTTP-сервер"),
        ];
        let known_ids: Vec<&str> = known.iter().map(|(id, _)| *id).collect();
        for p in &self.params {
            if !known_ids.contains(&p.category.as_str()) {
                known.push((Box::leak(p.category.clone().into_boxed_str()), "(другое)"));
            }
        }
        known
    }

}

/// User's runtime choice for a parameter.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ParamEntry {
    pub enabled: bool,
    pub value: Option<serde_json::Value>,
}

/// Enabled/disabled + value per parameter id. Disabled parameters are not
/// passed to the command line at all (llama.cpp applies its own default).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ParamState {
    pub entries: BTreeMap<String, ParamEntry>,
}

impl ParamState {
    /// Initialize state from the catalog defaults.
    #[allow(dead_code)] // used in unit tests
    pub fn from_catalog(catalog: &ParamsCatalog) -> Self {
        let mut entries = BTreeMap::new();
        for def in &catalog.params {
            entries.insert(
                def.id.clone(),
                ParamEntry {
                    enabled: def.enabled_default,
                    value: def.default.clone(),
                },
            );
        }
        Self { entries }
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.entries.get(id).is_some_and(|e| e.enabled)
    }

    pub fn set(&mut self, id: &str, enabled: bool) {
        let entry = self.entries.entry(id.to_string()).or_default();
        entry.enabled = enabled;
    }

    pub fn set_value(&mut self, id: &str, value: serde_json::Value) {
        let entry = self.entries.entry(id.to_string()).or_default();
        entry.value = Some(value);
    }

    /// Fill in defaults for parameters missing from persisted state
    /// (e.g. after a catalog update).
    pub fn merge_defaults(&mut self, catalog: &ParamsCatalog) {
        for def in &catalog.params {
            self.entries.entry(def.id.clone()).or_insert_with(|| ParamEntry {
                enabled: def.enabled_default,
                value: def.default.clone(),
            });
        }
    }
}

/// Serialize enabled parameters into CLI arguments, preserving catalog order.
pub fn to_args(catalog: &ParamsCatalog, state: &ParamState) -> Vec<String> {
    let mut args = Vec::new();
    for def in &catalog.params {
        let Some(entry) = state.entries.get(&def.id) else {
            continue;
        };
        if !entry.enabled {
            continue;
        }
        match def.kind {
            ParamKind::Bool => args.push(def.flag.clone()),
            ParamKind::Int | ParamKind::Float | ParamKind::Enum | ParamKind::String | ParamKind::Path => {
                let Some(value) = entry.value.as_ref().or(def.default.as_ref()) else {
                    continue;
                };
                let text = match value {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    other => other.to_string(),
                };
                args.push(def.flag.clone());
                args.push(text);
            }
        }
    }
    args
}

/// Validate values against catalog constraints. Returns a list of problems
/// in a human-readable form (empty = everything is fine).
pub fn validate(catalog: &ParamsCatalog, state: &ParamState) -> Vec<String> {
    let mut problems = Vec::new();
    for def in &catalog.params {
        let Some(entry) = state.entries.get(&def.id) else {
            continue;
        };
        if !entry.enabled {
            continue;
        }
        // Bool flags have no value — being enabled is all that matters.
        if def.kind == ParamKind::Bool {
            continue;
        }
        let Some(value) = entry.value.as_ref().or(def.default.as_ref()) else {
            problems.push(format!("«{}»: не задано значение", def.name));
            continue;
        };
        match def.kind {
            ParamKind::Int | ParamKind::Float => {
                let Some(number) = value.as_f64() else {
                    problems.push(format!("«{}»: ожидалось число", def.name));
                    continue;
                };
                if let Some(min) = def.min && number < min {
                    problems.push(format!("«{}»: {} < минимума {}", def.name, number, min));
                }
                if let Some(max) = def.max && number > max {
                    problems.push(format!("«{}»: {} > максимума {}", def.name, number, max));
                }
            }
            ParamKind::Enum => {
                let text = value.as_str().unwrap_or_default();
                if !def.options.is_empty() && !def.options.iter().any(|o| o == text) {
                    problems.push(format!(
                        "«{}»: «{}» не из списка {}",
                        def.name,
                        text,
                        def.options.join(", ")
                    ));
                }
            }
            ParamKind::String | ParamKind::Path | ParamKind::Bool => {}
        }
    }
    problems
}

/// Catalog bundled with the application binary.
pub fn bundled_catalog() -> ParamsCatalog {
    serde_json::from_str(include_str!("../assets/params_catalog.json"))
        .expect("bundled params catalog must be valid")
}

/// Download a catalog from a remote source (raw JSON URL).
pub fn fetch_catalog(url: &str) -> Result<ParamsCatalog, String> {
    let mut response = ureq::get(url)
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .call()
        .map_err(|e| format!("HTTP-запрос: {e}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("чтение ответа: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("разбор JSON: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_is_valid_and_unique() {
        let catalog = bundled_catalog();
        assert!(!catalog.params.is_empty());
        let mut ids: Vec<&str> = catalog.params.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "дублирующиеся id в каталоге");
        for def in &catalog.params {
            assert!(def.flag.starts_with('-'), "флаг без дефиса: {}", def.flag);
            if def.kind == ParamKind::Enum {
                assert!(!def.options.is_empty(), "enum без опций: {}", def.id);
            }
        }
    }

    #[test]
    fn to_args_respects_enabled_and_order() {
        let catalog = bundled_catalog();
        let mut state = ParamState::from_catalog(&catalog);
        // Everything disabled → no args at all.
        for def in &catalog.params {
            state.set(&def.id, false);
        }
        assert!(to_args(&catalog, &state).is_empty());

        // Enable ctx-size with value → ["--ctx-size", "8192"].
        state.set("ctx-size", true);
        state.set_value("ctx-size", serde_json::json!(65536));
        // Bool param: only the flag.
        state.set("jinja", true);
        let args = to_args(&catalog, &state);
        assert!(args.contains(&"--jinja".to_string()));
        let pos = args.iter().position(|a| a == "--ctx-size").unwrap();
        assert_eq!(args[pos + 1], "65536");
        // Catalog order: ctx-size comes before jinja.
        assert!(pos < args.iter().position(|a| a == "--jinja").unwrap());
    }

    #[test]
    fn validate_catches_range_and_enum() {
        let catalog = bundled_catalog();
        let mut state = ParamState::from_catalog(&catalog);
        for def in &catalog.params {
            state.set(&def.id, false);
        }
        state.set("ctx-size", true);
        state.set_value("ctx-size", serde_json::json!(-5));
        state.set("cache-type-k", true);
        state.set_value("cache-type-k", serde_json::json!("bogus"));
        state.set("temp", true);
        state.set_value("temp", serde_json::json!(3.5));
        let problems = validate(&catalog, &state);
        assert_eq!(problems.len(), 3, "ожидались 3 проблемы: {problems:?}");
    }

    #[test]
    fn disabled_params_are_not_validated() {
        let catalog = bundled_catalog();
        let mut state = ParamState::from_catalog(&catalog);
        state.set("temp", false);
        state.set_value("temp", serde_json::json!(99.0));
        assert!(validate(&catalog, &state).is_empty());
    }
}
