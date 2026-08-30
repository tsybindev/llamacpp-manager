//! Клиент GitHub Releases для сборок llama.cpp.
//!
//! Чистая работа с API: типы, разбор JSON и фильтрация ассетов.
//! Локальный кэш и скачивание — в `builds.rs` и слоях выше.

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
}

/// Архитектура процессора.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
}

impl Arch {
    pub fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Arm64)
        } else {
            None
        }
    }
}

/// Бэкенд llama.cpp внутри сборки.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Cpu,
    Vulkan,
    Cuda,
    Rocm,
    Sycl,
    Openvino,
    Opencl,
    /// Неопознанный бэкенд (ручной выбор из списка всех файлов).
    Other,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Vulkan => "Vulkan",
            Self::Cuda => "CUDA",
            Self::Rocm => "ROCm",
            Self::Sycl => "SYCL",
            Self::Openvino => "OpenVINO",
            Self::Opencl => "OpenCL",
            Self::Other => "Другое",
        }
    }
}

/// Результат классификации файла-ассета по имени.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BuildKind {
    /// None — платформа не та, для которой это приложение (macOS, Android).
    pub os: Option<TargetOs>,
    /// None — архитектура не распознана (например, s390x).
    pub arch: Option<Arch>,
    pub backend: Backend,
}

/// Доступная для скачивания сборка: ассет + его классификация.
#[derive(Clone, Debug, PartialEq)]
pub struct BuildAsset {
    pub asset: Asset,
    pub tag: String,
    pub os: Option<TargetOs>,
    pub arch: Option<Arch>,
    pub backend: Backend,
    /// Для Windows CUDA: отдельный архив cudart-*.zip с рантаймом,
    /// распаковывается в тот же каталог.
    pub runtime_asset: Option<Asset>,
}

impl BuildAsset {
    /// Имя каталога для этой сборки в локальной библиотеке.
    pub fn dir_name(&self) -> String {
        let stem = self
            .asset
            .name
            .trim_end_matches(".zip")
            .trim_end_matches(".tar.gz");
        stem.replace("-bin-", "-")
    }
}

/// Классифицировать файл-ассета по имени (формат релизов 2025+):
/// `llama-b10688-bin-ubuntu-vulkan-x64.tar.gz`, `llama-b10688-bin-win-cpu-x64.zip`.
/// Не сборки llama-server (ui, xcframework, cudart, исходники) → None.
pub fn classify_asset(name: &str) -> Option<BuildKind> {
    let name = name.to_ascii_lowercase();
    if !name.starts_with("llama-") || !name.contains("-bin-") || name.contains("cudart") {
        return None;
    }
    if !name.ends_with(".zip") && !name.ends_with(".tar.gz") {
        return None;
    }
    let os = if name.contains("-win-") {
        Some(TargetOs::Windows)
    } else if name.contains("ubuntu") {
        Some(TargetOs::Linux)
    } else {
        None // macOS, Android — вне scope приложения.
    };
    let arch = if name.contains("-arm64") {
        Some(Arch::Arm64)
    } else if name.contains("x64") {
        Some(Arch::X64)
    } else {
        None
    };
    let backend = if name.contains("vulkan") {
        Backend::Vulkan
    } else if name.contains("cuda") {
        Backend::Cuda
    } else if name.contains("rocm") {
        Backend::Rocm
    } else if name.contains("sycl") {
        Backend::Sycl
    } else if name.contains("openvino") {
        Backend::Openvino
    } else if name.contains("opencl") {
        Backend::Opencl
    } else {
        Backend::Cpu
    };
    Some(BuildKind { os, arch, backend })
}

/// Собрать плоский список скачиваемых сборок из всех релизов.
/// Для Windows CUDA сразу подставляется парный архив cudart.
pub fn buildable_assets(releases: &[Release]) -> Vec<BuildAsset> {
    let mut out = Vec::new();
    for release in releases {
        for asset in &release.assets {
            let Some(kind) = classify_asset(&asset.name) else {
                continue;
            };
            // cudart-llama-bin-win-cuda-12.4-x64.zip ↔ llama-b10688-bin-win-cuda-12.4-x64.zip
            // (в имени cudart-архива нет тега версии).
            let runtime_asset = if kind.backend == Backend::Cuda {
                let base = asset.name.replace(&format!("-{}-", release.tag), "-");
                let wanted = format!("cudart-{base}");
                release
                    .assets
                    .iter()
                    .find(|candidate| candidate.name == wanted)
                    .cloned()
            } else {
                None
            };
            out.push(BuildAsset {
                asset: asset.clone(),
                tag: release.tag.clone(),
                os: kind.os,
                arch: kind.arch,
                backend: kind.backend,
                runtime_asset,
            });
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
        // Современные имена релизов (2025+): tar.gz для Linux, zip для Windows.
        let kind = classify_asset("llama-b10688-bin-ubuntu-vulkan-x64.tar.gz").expect("vulkan");
        assert_eq!(kind.os, Some(TargetOs::Linux));
        assert_eq!(kind.arch, Some(Arch::X64));
        assert_eq!(kind.backend, Backend::Vulkan);

        let kind = classify_asset("llama-b10688-bin-ubuntu-x64.tar.gz").expect("cpu linux");
        assert_eq!(kind.os, Some(TargetOs::Linux));
        assert_eq!(kind.backend, Backend::Cpu);

        let kind = classify_asset("llama-b10688-bin-ubuntu-rocm-7.14-x64.tar.gz").expect("rocm");
        assert_eq!(kind.backend, Backend::Rocm);

        let kind = classify_asset("llama-b10688-bin-win-cuda-12.4-x64.zip").expect("cuda win");
        assert_eq!(kind.os, Some(TargetOs::Windows));
        assert_eq!(kind.arch, Some(Arch::X64));
        assert_eq!(kind.backend, Backend::Cuda);

        let kind = classify_asset("llama-b10688-bin-win-vulkan-x64.zip").expect("vulkan win");
        assert_eq!(kind.backend, Backend::Vulkan);

        let kind = classify_asset("llama-b10688-bin-win-cpu-arm64.zip").expect("arm64");
        assert_eq!(kind.arch, Some(Arch::Arm64));

        // macOS/Android — вне scope: ос None (остаются в ручном списке).
        let kind = classify_asset("llama-b10688-bin-macos-x64.tar.gz").expect("macos kind");
        assert_eq!(kind.os, None);

        // Не наши файлы.
        assert_eq!(classify_asset("llama-b10688-ui.tar.gz"), None);
        assert_eq!(classify_asset("llama-b10688-xcframework.zip"), None);
        assert_eq!(classify_asset("cudart-llama-bin-win-cuda-12.4-x64.zip"), None);
        assert_eq!(classify_asset("llama-b10688-src.zip"), None);
        assert_eq!(classify_asset("random-file.zip"), None);
    }

    #[test]
    fn buildable_assets_filters_and_keeps_tag() {
        let releases = vec![Release {
            tag: "b10688".into(),
            published_at: String::new(),
            assets: vec![
                Asset {
                    name: "llama-b10688-bin-ubuntu-x64.tar.gz".into(),
                    browser_download_url: "https://example.com/a.tar.gz".into(),
                    size: 1,
                },
                Asset {
                    name: "llama-b10688-bin-win-vulkan-x64.zip".into(),
                    browser_download_url: "https://example.com/b.zip".into(),
                    size: 2,
                },
                Asset {
                    name: "llama-b10688-bin-win-cuda-12.4-x64.zip".into(),
                    browser_download_url: "https://example.com/c.zip".into(),
                    size: 3,
                },
                Asset {
                    name: "cudart-llama-bin-win-cuda-12.4-x64.zip".into(),
                    browser_download_url: "https://example.com/d.zip".into(),
                    size: 4,
                },
                Asset {
                    name: "llama-b10688-ui.tar.gz".into(),
                    browser_download_url: "https://example.com/e.zip".into(),
                    size: 5,
                },
            ],
        }];
        let assets = buildable_assets(&releases);
        assert_eq!(assets.len(), 3);
        assert_eq!(assets[0].tag, "b10688");
        assert_eq!(assets[0].backend, Backend::Cpu);
        assert_eq!(assets[0].os, Some(TargetOs::Linux));
        assert_eq!(assets[1].backend, Backend::Vulkan);
        assert_eq!(assets[0].dir_name(), "llama-b10688-ubuntu-x64");
        assert_eq!(assets[1].dir_name(), "llama-b10688-win-vulkan-x64");
        // Для CUDA подставлен парный cudart-архив.
        assert_eq!(
            assets[2].runtime_asset.as_ref().map(|a| a.name.as_str()),
            Some("cudart-llama-bin-win-cuda-12.4-x64.zip")
        );
        assert_eq!(assets[1].runtime_asset, None);
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
