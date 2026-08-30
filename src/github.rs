//! Клиент GitHub Releases для сборок llama.cpp.
//!
//! Чистая работа с API: типы, разбор JSON и фильтрация ассетов.
//! Локальный кэш и скачивание — в `builds.rs` и слоях выше.
// Разрешение снимется, когда модуль начнёт использоваться слоями выше.
#![allow(dead_code)]

use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const RELEASES_URL: &str =
    "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=30";
/// GitHub API требует заголовок User-Agent, иначе отвечает 403.
const USER_AGENT: &str = "llamacpp-manager";

/// Файл в релизе (только нужные поля GitHub API).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

/// Релиз llama.cpp: тег (например `b10635`) и список файлов-ассетов.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Release {
    #[serde(rename = "tag_name")]
    pub tag: String,
    #[serde(default)]
    pub published_at: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

/// Целевая операционная система сборки.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetOs {
    Windows,
    Linux,
}

impl TargetOs {
    /// ОС текущей машины — фильтруем список сборок под неё.
    pub fn current() -> Option<Self> {
        if cfg!(target_os = "windows") {
            Some(Self::Windows)
        } else if cfg!(target_os = "linux") {
            Some(Self::Linux)
        } else {
            None
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Windows => "Windows",
            Self::Linux => "Linux",
        }
    }
}

/// Бэкенд llama.cpp внутри сборки.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Vulkan,
    Cuda,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Vulkan => "Vulkan",
            Self::Cuda => "CUDA",
        }
    }
}

/// Доступная для скачивания сборка: ассет + его классификация.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildAsset {
    pub asset: Asset,
    pub tag: String,
    pub os: TargetOs,
    pub backend: Backend,
}

impl BuildAsset {
    /// Имя каталога для этой сборки в локальной библиотеке.
    pub fn dir_name(&self) -> String {
        // llama-b10635-bin-win-cuda-12.4-x64.zip → llama-b10635-win-cuda-12.4-x64
        let stem = self.asset.name.trim_end_matches(".zip");
        stem.replace("-bin-", "-")
    }
}

/// Определить ОС и бэкенд по имени файла-ассета.
/// Неузнаваемые имена (исходники, cudart-библиотеки, macOS) → None.
pub fn classify_asset(name: &str) -> Option<(TargetOs, Backend)> {
    let name = name.to_ascii_lowercase();
    if !name.starts_with("llama-") || name.contains("cudart") || !name.ends_with(".zip") {
        return None;
    }
    if let Some(rest) = name.strip_prefix("llama-") {
        let os = if rest.contains("-win-") {
            TargetOs::Windows
        } else if rest.contains("ubuntu") {
            TargetOs::Linux
        } else {
            return None;
        };
        let backend = if rest.contains("-vulkan") {
            Backend::Vulkan
        } else if rest.contains("-cuda") {
            Backend::Cuda
        } else {
            Backend::Cpu
        };
        return Some((os, backend));
    }
    None
}

/// Собрать плоский список скачиваемых сборок из всех релизов.
pub fn buildable_assets(releases: &[Release]) -> Vec<BuildAsset> {
    let mut out = Vec::new();
    for release in releases {
        for asset in &release.assets {
            if let Some((os, backend)) = classify_asset(&asset.name) {
                out.push(BuildAsset {
                    asset: asset.clone(),
                    tag: release.tag.clone(),
                    os,
                    backend,
                });
            }
        }
    }
    out
}

/// Разобрать JSON-ответ `/releases`. Отдельная функция — для юнит-тестов.
pub fn parse_releases(json: &str) -> Result<Vec<Release>> {
    serde_json::from_str(json).context("разбор JSON списка релизов")
}

/// Загрузить список последних релизов llama.cpp с GitHub API.
pub fn fetch_releases() -> Result<Vec<Release>> {
    let mut response = ureq::get(RELEASES_URL)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .call()
        .context("запрос списка релизов GitHub")?;
    let text = response
        .body_mut()
        .read_to_string()
        .context("чтение ответа GitHub")?;
    parse_releases(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_releases_sample() {
        let json = r#"[
            {
                "tag_name": "b10635",
                "published_at": "2025-08-01T00:00:00Z",
                "assets": [
                    {
                        "name": "llama-b10635-bin-win-cuda-12.4-x64.zip",
                        "browser_download_url": "https://example.com/cuda.zip",
                        "size": 123456
                    },
                    {
                        "name": "llama-b10635-bin-ubuntu-x64.zip",
                        "browser_download_url": "https://example.com/cpu.zip",
                        "size": 234567
                    }
                ]
            },
            {
                "tag_name": "b10000",
                "assets": []
            }
        ]"#;
        let releases = parse_releases(json).expect("разбор");
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].tag, "b10635");
        assert_eq!(releases[0].assets.len(), 2);
        assert_eq!(releases[0].assets[0].size, 123456);
        assert_eq!(releases[1].assets.len(), 0);
    }

    #[test]
    fn classify_known_and_unknown_assets() {
        let (os, backend) =
            classify_asset("llama-b10635-bin-win-cuda-12.4-x64.zip").expect("cuda win");
        assert_eq!(os, TargetOs::Windows);
        assert_eq!(backend, Backend::Cuda);

        let (os, backend) = classify_asset("llama-b10635-bin-win-vulkan-x64.zip").expect("vulkan");
        assert_eq!(os, TargetOs::Windows);
        assert_eq!(backend, Backend::Vulkan);

        let (os, backend) = classify_asset("llama-b10635-bin-win-cpu-x64.zip").expect("cpu win");
        assert_eq!(os, TargetOs::Windows);
        assert_eq!(backend, Backend::Cpu);

        let (os, backend) = classify_asset("llama-b10635-bin-ubuntu-x64.zip").expect("linux cpu");
        assert_eq!(os, TargetOs::Linux);
        assert_eq!(backend, Backend::Cpu);

        // Не наши файлы: исходники, cudart, пакеты исходного кода, tar.gz.
        assert_eq!(classify_asset("llama-b10635-ubuntu-x64.tar.gz"), None);
        assert_eq!(classify_asset("cudart-llama-bin-win-cuda-12.4-x64.zip"), None);
        assert_eq!(classify_asset("llama-b10635-src.zip"), None);
        assert_eq!(classify_asset("random-file.zip"), None);
    }

    #[test]
    fn buildable_assets_filters_and_keeps_tag() {
        let releases = vec![Release {
            tag: "b10635".into(),
            published_at: String::new(),
            assets: vec![
                Asset {
                    name: "llama-b10635-bin-ubuntu-x64.zip".into(),
                    browser_download_url: "https://example.com/a.zip".into(),
                    size: 1,
                },
                Asset {
                    name: "llama-b10635-bin-win-vulkan-x64.zip".into(),
                    browser_download_url: "https://example.com/b.zip".into(),
                    size: 2,
                },
                Asset {
                    name: "cudart-llama-bin-win-cuda-12.4-x64.zip".into(),
                    browser_download_url: "https://example.com/c.zip".into(),
                    size: 3,
                },
            ],
        }];
        let assets = buildable_assets(&releases);
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].tag, "b10635");
        assert_eq!(assets[0].backend, Backend::Cpu);
        assert_eq!(assets[1].backend, Backend::Vulkan);
        assert_eq!(assets[0].dir_name(), "llama-b10635-ubuntu-x64");
        assert_eq!(assets[1].dir_name(), "llama-b10635-win-vulkan-x64");
    }

    /// Сетевой тест: запускается явно (`cargo test -- --ignored`).
    #[test]
    #[ignore = "требует доступ к api.github.com"]
    fn fetch_releases_live() {
        let releases = fetch_releases().expect("сеть");
        assert!(!releases.is_empty());
        assert!(releases[0].tag.starts_with('b'));
        assert!(!buildable_assets(&releases).is_empty());
    }
}
