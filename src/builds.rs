//! Локальная библиотека сборок llama.cpp: скачивание релизов с GitHub,
//! распаковка zip и кэш списка версий.

use std::fs::{self, File};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::github::{self, Backend, BuildAsset, Release};

const USER_AGENT: &str = "llamacpp-manager";
const RELEASES_CACHE_FILE: &str = "releases-cache.json";
/// Кэш списка релизов считается свежим сутки; потом обновляется в фоне.
const RELEASES_CACHE_AGE_SECS: u64 = 24 * 60 * 60;

/// Событие прогресса для UI во время скачивания и распаковки.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    Downloading { downloaded: u64, total: u64 },
    Extracting,
}

/// Сборка, установленная в локальную библиотеку (подкаталог в builds_dir).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InstalledBuild {
    pub dir: PathBuf,
    pub tag: String,
    pub backend: Backend,
}

impl InstalledBuild {
    pub fn label(&self) -> String {
        format!("{} · {}", self.tag, self.backend.label())
    }

    /// Путь к llama-server внутри каталога сборки (если найден).
    pub fn server_binary(&self) -> Option<PathBuf> {
        server_binary_in(&self.dir)
    }
}

/// Найти llama-server[.exe] в каталоге сборки.
pub fn server_binary_in(dir: &Path) -> Option<PathBuf> {
    let exe = if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    let direct = dir.join(exe);
    if direct.is_file() {
        return Some(direct);
    }
    // В zip-архивах релизов бинарник лежит в корне, но подстрахуемся
    // одн уровнем вложенности (папка внутри архива).
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let nested = entry.path().join(exe);
        if nested.is_file() {
            return Some(nested);
        }
    }
    None
}

/// Разобрать имя каталога сборки: `llama-b10635-win-cuda-12.4-x64`
/// → ("b10635", Cuda). Возвращает None для чужих каталогов.
pub fn parse_dir_name(name: &str) -> Option<(String, Backend)> {
    let rest = name.strip_prefix("llama-")?;
    let dash = rest.find('-')?;
    let tag = &rest[..dash];
    if tag.is_empty() {
        return None;
    }
    let backend = if rest.contains("vulkan") {
        Backend::Vulkan
    } else if rest.contains("cuda") {
        Backend::Cuda
    } else {
        Backend::Cpu
    };
    Some((tag.to_string(), backend))
}

/// Локальная библиотека сборок в каталоге builds_dir.
pub struct BuildsStore {
    dir: PathBuf,
}

impl BuildsStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Список установленных сборок, отсортированный по имени каталога.
    pub fn installed(&self) -> Vec<InstalledBuild> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut builds = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if let Some((tag, backend)) = parse_dir_name(name) {
                builds.push(InstalledBuild {
                    dir: path,
                    tag,
                    backend,
                });
            }
        }
        builds.sort_by(|a, b| b.dir.cmp(&a.dir));
        builds
    }

    /// Каталог для указанной сборки (ещё не обязательно установлена).
    pub fn dir_for(&self, asset: &BuildAsset) -> PathBuf {
        self.dir.join(asset.dir_name())
    }

    /// Скачивание и установка сборки. Блокирующая — вызывать из фонового
    /// потока; прогресс отдаётся через колбэк. Повторная установка
    /// существующей сборки заменяет её содержимое.
    pub fn install(
        &self,
        asset: &BuildAsset,
        mut progress: impl FnMut(Progress),
    ) -> Result<InstalledBuild> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("не удалось создать каталог {}", self.dir.display()))?;

        let zip_path = self
            .dir
            .join(format!(".download-{}-{}.zip", asset.dir_name(), std::process::id()));
        download_file(&asset.asset.browser_download_url, &zip_path, |d, t| {
            progress(Progress::Downloading { downloaded: d, total: t })
        })
        .with_context(|| format!("скачивание {}", asset.asset.name))?;

        progress(Progress::Extracting);
        let result = install_zip(&zip_path, self, asset);
        let _ = fs::remove_file(&zip_path);
        result
    }
}

/// Скачать файл по URL с отчётом о прогрессе (блокирующе).
pub fn download_file(
    url: &str,
    dest: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<()> {
    // Без timeout_global: он накрыл бы и чтение тела, а сборки — сотни мегабайт.
    let mut response = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .call()
        .context("HTTP-запрос")?;

    let total = response
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    let mut reader = response.body_mut().as_reader();
    let mut file = File::create(dest).with_context(|| {
        format!("не удалось создать файл {}", dest.display())
    })?;

    let mut downloaded: u64 = 0;
    let mut last_reported: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut chunk).context("чтение тела ответа")?;
        if read == 0 {
            break;
        }
        file.write_all(&chunk[..read]).context("запись файла")?;
        downloaded += read as u64;
        // Отчитываемся не чаще раза в мегабайт, чтобы не перегружать канал UI.
        if downloaded - last_reported >= 1024 * 1024 {
            last_reported = downloaded;
            progress(downloaded, total);
        }
    }
    file.flush().context("запись файла")?;
    progress(downloaded, if total > 0 { total } else { downloaded });
    Ok(())
}

/// Распаковать скачанный zip релиза в каталог сборки библиотеки.
/// Устанавливает через staging-каталог: распаковка → замена целевого каталога.
fn install_zip(zip_path: &Path, store: &BuildsStore, asset: &BuildAsset) -> Result<InstalledBuild> {
    let file = File::open(zip_path)
        .with_context(|| format!("не удалось открыть {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("чтение zip-архива")?;

    let final_dir = store.dir_for(asset);
    let staging = store.dir.join(format!(".staging-{}", asset.dir_name()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .with_context(|| format!("не удалось создать каталог {}", staging.display()))?;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("чтение записи zip")?;
        // enclosed_name отсекает zip-slip: пути, выходящие за каталог распаковки.
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        let target = staging.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target).with_context(|| {
                format!("не удалось создать каталог {}", target.display())
            })?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("не удалось создать каталог {}", parent.display())
                })?;
            }
            let mut out = File::create(&target)
                .with_context(|| format!("не удалось создать файл {}", target.display()))?;
            std::io::copy(&mut entry, &mut out)
                .with_context(|| format!("распаковка {}", relative.display()))?;
        }
    }

    if server_binary_in(&staging).is_none() {
        let _ = fs::remove_dir_all(&staging);
        bail!("в архиве {} не найден llama-server", asset.asset.name);
    }

    // Заменяем старую установку той же версии.
    let _ = fs::remove_dir_all(&final_dir);
    fs::rename(&staging, &final_dir).with_context(|| {
        format!(
            "переименование {} → {}",
            staging.display(),
            final_dir.display()
        )
    })?;

    let (tag, _) = parse_dir_name(&asset.dir_name())
        .with_context(|| format!("неожиданное имя каталога {}", asset.dir_name()))?;
    Ok(InstalledBuild {
        dir: final_dir,
        tag,
        backend: asset.backend,
    })
}

// --- Кэш списка релизов ---

#[derive(Serialize, Deserialize)]
struct ReleasesCache {
    fetched_at: u64,
    releases: Vec<Release>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Список релизов с GitHub с локальным кэшем на сутки.
/// При недоступности сети возвращает устаревший кэш, если он есть.
pub fn fetch_releases_cached(cache_dir: &Path, force_refresh: bool) -> Result<Vec<Release>> {
    fetch_releases_cached_with(cache_dir, force_refresh, github::fetch_releases)
}

/// То же с подменяемой сетевой функцией — для детерминированных тестов.
fn fetch_releases_cached_with(
    cache_dir: &Path,
    force_refresh: bool,
    fetch: impl Fn() -> Result<Vec<Release>>,
) -> Result<Vec<Release>> {
    fs::create_dir_all(cache_dir).with_context(|| {
        format!("не удалось создать каталог {}", cache_dir.display())
    })?;
    let cache_path = cache_dir.join(RELEASES_CACHE_FILE);

    if !force_refresh
        && let Ok(cached) = read_cache(&cache_path)
        && now_unix().saturating_sub(cached.fetched_at) < RELEASES_CACHE_AGE_SECS
    {
        return Ok(cached.releases);
    }

    match fetch() {
        Ok(releases) => {
            let cache = ReleasesCache {
                fetched_at: now_unix(),
                releases: releases.clone(),
            };
            if let Ok(json) = serde_json::to_string(&cache) {
                let tmp = cache_path.with_extension("json.tmp");
                if fs::write(&tmp, &json).is_ok() {
                    let _ = fs::rename(&tmp, &cache_path);
                }
            }
            Ok(releases)
        }
        Err(e) => {
            // Сеть недоступна — отдаём устаревший кэш, если он существует.
            if let Ok(cached) = read_cache(&cache_path) {
                log::warn!("GitHub недоступен, использую кэш релизов: {e:#}");
                return Ok(cached.releases);
            }
            Err(e)
        }
    }
}

fn read_cache(path: &Path) -> Result<ReleasesCache> {
    let text = fs::read_to_string(path).context("чтение кэша")?;
    serde_json::from_str(&text).context("разбор кэша релизов")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::TargetOs;
    use std::io::Write as _;

    fn temp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "llamacpp-manager-builds-{label}-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_asset(name: &str, url: &str) -> BuildAsset {
        BuildAsset {
            asset: github::Asset {
                name: name.to_string(),
                browser_download_url: url.to_string(),
                size: 0,
            },
            tag: "b10635".into(),
            os: TargetOs::Linux,
            backend: Backend::Cpu,
        }
    }

    /// Собрать тестовый zip с бинарником и вложенным файлом.
    fn make_test_zip(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("llama-server", options).unwrap();
        zip.write_all(b"#!/bin/sh\necho mock llama-server\n").unwrap();
        zip.start_file("sub/libexample.so", options).unwrap();
        zip.write_all(b"binary-blob").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn parse_dir_name_variants() {
        assert_eq!(
            parse_dir_name("llama-b10635-ubuntu-x64"),
            Some(("b10635".into(), Backend::Cpu))
        );
        assert_eq!(
            parse_dir_name("llama-b10635-win-cuda-12.4-x64"),
            Some(("b10635".into(), Backend::Cuda))
        );
        assert_eq!(
            parse_dir_name("llama-b10635-win-vulkan-x64"),
            Some(("b10635".into(), Backend::Vulkan))
        );
        // Чужие каталоги и пустые теги не распознаются.
        assert_eq!(parse_dir_name("releases-cache.json"), None);
        assert_eq!(parse_dir_name("llama--ubuntu"), None);
    }

    #[test]
    fn install_zip_extracts_and_replaces() {
        let root = temp_root("install");
        let store = BuildsStore::new(root.clone());
        let asset = sample_asset("llama-b10635-bin-ubuntu-x64.zip", "https://unused");
        make_test_zip(&root.join("test.zip"));

        let build = install_zip(&root.join("test.zip"), &store, &asset).expect("установка");
        assert_eq!(build.tag, "b10635");
        assert_eq!(build.backend, Backend::Cpu);
        assert_eq!(build.dir, store.dir_for(&asset));
        assert_eq!(
            fs::read(build.dir.join("llama-server")).unwrap(),
            b"#!/bin/sh\necho mock llama-server\n"
        );
        assert!(build.dir.join("sub/libexample.so").is_file());
        assert!(build.server_binary().is_some());

        // Повторная установка (обновление версии) заменяет каталог целиком.
        make_test_zip(&root.join("test.zip"));
        install_zip(&root.join("test.zip"), &store, &asset).expect("повторная установка");
        assert!(build.server_binary().is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn install_zip_without_server_binary_fails_cleanly() {
        let root = temp_root("noserver");
        let store = BuildsStore::new(root.clone());
        let asset = sample_asset("llama-b99999-bin-ubuntu-x64.zip", "https://unused");

        let file = File::create(root.join("empty.zip")).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options =
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        zip.start_file("readme.txt", options).unwrap();
        zip.write_all(b"no binaries here").unwrap();
        zip.finish().unwrap();

        let err = install_zip(&root.join("empty.zip"), &store, &asset);
        assert!(err.is_err());
        // staging-каталог и целевой каталог не должны остаться.
        assert!(!store.dir_for(&asset).exists());
        assert!(!store.dir.join(".staging-llama-b99999-ubuntu-x64").exists());

        std::fs::remove_dir_all(&root).ok();
    }

    /// Сетевой тест: потоковое скачивание маленького файла по HTTP.
    #[test]
    #[ignore = "требует доступ к сети"]
    fn download_file_live() {
        let root = temp_root("download-live");
        let dest = root.join("params_catalog.json");
        let mut total_seen = 0u64;
        download_file(
            "https://raw.githubusercontent.com/tsybindev/llamacpp-manager/main/assets/params_catalog.json",
            &dest,
            |downloaded, total| {
                total_seen = total;
                assert!(downloaded <= total.max(1));
            },
        )
        .expect("скачивание");
        let text = fs::read_to_string(&dest).expect("файл");
        assert!(text.contains("\"version\""));
        let _ = total_seen;
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn releases_cache_roundtrip_and_stale_fallback() {
        let root = temp_root("cache");
        let releases = vec![github::Release {
            tag: "b10635".into(),
            published_at: "2025-08-01T00:00:00Z".into(),
            assets: vec![],
        }];

        // Свежий кэш: fetch не вызывается (force_refresh=false).
        let cache = ReleasesCache {
            fetched_at: now_unix(),
            releases: releases.clone(),
        };
        fs::write(
            root.join(RELEASES_CACHE_FILE),
            serde_json::to_string(&cache).unwrap(),
        )
        .unwrap();
        let got = fetch_releases_cached(&root, false).expect("свежий кэш");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].tag, "b10635");

        // Устаревший кэш + отказ сети → отдаётся устаревший кэш.
        let cache = ReleasesCache {
            fetched_at: now_unix() - RELEASES_CACHE_AGE_SECS - 10,
            releases: releases.clone(),
        };
        fs::write(
            root.join(RELEASES_CACHE_FILE),
            serde_json::to_string(&cache).unwrap(),
        )
        .unwrap();
        let got = fetch_releases_cached_with(&root, true, || {
            Err(anyhow::anyhow!("сеть недоступна"))
        })
        .expect("устаревший кэш при отказе сети");
        assert_eq!(got[0].tag, "b10635");

        // Нет кэша + отказ сети → ошибка.
        let empty = temp_root("cache-empty");
        let err = fetch_releases_cached_with(&empty, true, || {
            Err(anyhow::anyhow!("сеть недоступна"))
        });
        assert!(err.is_err());

        // Обновление сети перезаписывает кэш.
        let fresh = vec![github::Release {
            tag: "b20000".into(),
            published_at: String::new(),
            assets: vec![],
        }];
        let got = fetch_releases_cached_with(&root, true, || Ok(fresh.clone())).expect("сеть ок");
        assert_eq!(got[0].tag, "b20000");

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&empty).ok();
    }
}
