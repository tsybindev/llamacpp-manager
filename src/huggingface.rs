//! Клиент HuggingFace API: поиск GGUF-моделей, список файлов репозитория,
//! скачивание (с докачкой) и разбор квантования из имени файла.
// Разрешение снимется, когда модуль начнёт использоваться UI.
#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::download::{self, DownloadRequest};

const API_BASE: &str = "https://huggingface.co";
const USER_AGENT: &str = "llamacpp-manager";

/// Найденная модель (репозиторий с GGUF).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HfModel {
    pub id: String,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub downloads: u64,
}

/// Файл в репозитории модели.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HfFile {
    pub path: String,
    #[serde(default)]
    pub size: u64,
}

/// URL для скачивания файла из репозитория (resolve — редирект на CDN).
pub fn download_url(repo: &str, path: &str) -> String {
    format!("{API_BASE}/{repo}/resolve/main/{path}")
}

/// Кодирование компонента URL (пробелы и спецсимволы → %XX).
pub fn url_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Поиск моделей по запросу (только репозитории с GGUF, сортировка по скачиваниям).
pub fn search_models(query: &str, token: Option<&str>) -> Result<Vec<HfModel>> {
    let url = format!(
        "{API_BASE}/api/models?search={}&filter=gguf&limit=30&sort=downloads&direction=-1",
        url_encode(query)
    );
    let text = get_text(&url, token)?;
    parse_search(&text)
}

/// Файлы .gguf в корне репозитория модели (mmproj/draft включаются —
/// это тоже gguf; каталоги отсекаются).
pub fn list_gguf_files(repo: &str, token: Option<&str>) -> Result<Vec<HfFile>> {
    // repo содержит разделитель «org/name» — кодировать его нельзя, только путь.
    let url = format!("{API_BASE}/api/models/{repo}/tree/main");
    let text = get_text(&url, token)?;
    parse_tree(&text)
}

/// Скачивание файла модели с докачкой при обрыве.
pub fn download_model_file(
    repo: &str,
    path: &str,
    dest: &std::path::Path,
    token: Option<&str>,
    progress: impl FnMut(u64, u64),
) -> Result<()> {
    let mut headers = vec![("User-Agent".to_string(), USER_AGENT.to_string())];
    if let Some(token) = token
        && !token.trim().is_empty()
    {
        headers.push(("Authorization".to_string(), format!("Bearer {}", token.trim())));
    }
    download::download_file(
        &DownloadRequest {
            url: download_url(repo, path),
            dest: dest.to_path_buf(),
            headers,
            resume: true,
        },
        progress,
    )
    .with_context(|| format!("скачивание {repo}/{path}"))
}

fn get_text(url: &str, token: Option<&str>) -> Result<String> {
    let mut req = ureq::get(url).header("User-Agent", USER_AGENT);
    if let Some(token) = token
        && !token.trim().is_empty()
    {
        req = req.header("Authorization", &format!("Bearer {}", token.trim()));
    }
    let mut response = req
        .config()
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .call()
        .with_context(|| format!("запрос {url}"))?;
    let text = response
        .body_mut()
        .read_to_string()
        .context("чтение ответа HuggingFace")?;
    Ok(text)
}

pub fn parse_search(json: &str) -> Result<Vec<HfModel>> {
    serde_json::from_str(json).context("разбор результатов поиска")
}

pub fn parse_tree(json: &str) -> Result<Vec<HfFile>> {
    let entries: Vec<serde_json::Value> =
        serde_json::from_str(json).context("разбор списка файлов")?;
    Ok(entries
        .iter()
        .filter_map(|entry| {
            let is_file = entry.get("type").and_then(|t| t.as_str()) == Some("file");
            let path = entry.get("path").and_then(|p| p.as_str())?;
            let size = entry.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            is_file.then(|| HfFile {
                path: path.to_string(),
                size,
            })
        })
        .filter(|file| file.path.to_ascii_lowercase().ends_with(".gguf"))
        .collect())
}

/// Определить квантование из имени GGUF-файла: «модель-Q4_K_M.gguf» → Q4_K_M.
/// Понимает префикс UD- (Unsloth Dynamic), IQ-кванты и форматы F16/BF16/F32/F8.
pub fn quant_from_filename(name: &str) -> Option<String> {
    let stem = name.strip_suffix(".gguf").unwrap_or(name).to_ascii_uppercase();
    let bytes = stem.as_bytes();

    // Первый фрагмент вида Q<цифра>… или IQ<цифра>… (QAT «Q» не цифра — мимо).
    for i in 0..bytes.len() {
        let is_quant_start = (bytes[i] == b'Q'
            && i + 1 < bytes.len()
            && bytes[i + 1].is_ascii_digit())
            || (bytes[i] == b'I'
                && i + 2 < bytes.len()
                && bytes[i + 1] == b'Q'
                && bytes[i + 2].is_ascii_digit());
        if !is_quant_start {
            continue;
        }
        let start = i;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
            end += 1;
        }
        let mut from = start;
        if from >= 3 && &stem[from - 3..from] == "UD-" {
            from -= 3;
        }
        return Some(stem[from..end].to_string());
    }
    // Без Q-квантов: FP-форматы.
    ["F16", "BF16", "F32", "F8"]
        .iter()
        .find_map(|fmt| stem.rfind(fmt).map(|_| fmt.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_search_sample() {
        let json = r#"[
            {"id": "google/gemma-3n-E4B-it-GGUF", "likes": 120, "downloads": 54321},
            {"id": "user/model", "likes": 0, "downloads": 0},
            {"id": "no-numbers"}
        ]"#;
        let models = parse_search(json).expect("разбор");
        assert_eq!(models.len(), 3);
        assert_eq!(models[0].id, "google/gemma-3n-E4B-it-GGUF");
        assert_eq!(models[0].downloads, 54321);
        assert_eq!(models[2].likes, 0);
    }

    #[test]
    fn parse_tree_filters_gguf_and_dirs() {
        let json = r#"[
            {"type": "file", "path": "gemma-Q4_K_M.gguf", "size": 100},
            {"type": "file", "path": "mmproj-F16.gguf", "size": 200},
            {"type": "file", "path": "readme.md", "size": 10},
            {"type": "directory", "path": "sub-Q4_K_M", "size": 0}
        ]"#;
        let files = parse_tree(json).expect("разбор");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].path, "gemma-Q4_K_M.gguf");
        assert_eq!(files[1].size, 200);
    }

    #[test]
    fn quant_detection() {
        assert_eq!(
            quant_from_filename("gemma-3n-E4B-it-QAT-UD-Q4_K_XL.gguf"),
            Some("UD-Q4_K_XL".to_string())
        );
        assert_eq!(
            quant_from_filename("model.Q4_K_M.gguf"),
            Some("Q4_K_M".to_string())
        );
        assert_eq!(
            quant_from_filename("mmproj-F16.gguf"),
            Some("F16".to_string())
        );
        assert_eq!(
            quant_from_filename("model.IQ4_XS.gguf"),
            Some("IQ4_XS".to_string())
        );
        assert_eq!(
            quant_from_filename("somefile.gguf"),
            None
        );
    }

    #[test]
    fn url_encoding_and_download_url() {
        assert_eq!(url_encode("qwen 3 gguf"), "qwen%203%20gguf");
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(
            download_url("org/repo", "model-Q4_K_M.gguf"),
            "https://huggingface.co/org/repo/resolve/main/model-Q4_K_M.gguf"
        );
    }

    /// Сетевые тесты: запускаются явно (`cargo test -- --ignored`).
    #[test]
    #[ignore = "требует доступ к huggingface.co"]
    fn search_live() {
        let models = search_models("gemma", None).expect("поиск");
        assert!(!models.is_empty());
        assert!(models.iter().any(|m| m.id.to_lowercase().contains("gguf")));
        let repo = &models[0].id;
        let files = list_gguf_files(repo, None).expect("файлы");
        assert!(!files.is_empty());
    }

    /// Сетевой тест механики скачивания: публичный маленький файл репозитория.
    #[test]
    #[ignore = "требует доступ к huggingface.co"]
    fn download_live_tiny_file() {
        let root = std::env::temp_dir().join(format!("hf-download-live-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let dest = root.join("config.json");
        download_model_file("openai-community/gpt2", "config.json", &dest, None, |_, _| {})
            .expect("скачивание");
        let size = std::fs::metadata(&dest).unwrap().len();
        assert!(size > 0);
        // Повторное скачивание с resume=true перезаписывает файл без ошибок.
        download_model_file("openai-community/gpt2", "config.json", &dest, None, |_, _| {})
            .expect("повторное скачивание");
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), size);
        std::fs::remove_dir_all(&root).ok();
    }
}
