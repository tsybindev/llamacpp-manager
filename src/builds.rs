//! Локальная библиотека сборок llama.cpp: скачивание релизов с GitHub,
//! распаковка zip и кэш списка версий.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Найти llama-server[.exe] в каталоге сборки (включая подкаталоги до 3 уровней —
/// глубина вложенности внутри архивов релизов различается между форматами).
pub fn server_binary_in(dir: &Path) -> Option<PathBuf> {
    let exe = if cfg!(target_os = "windows") {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    fn search(dir: &Path, exe: &str, depth: u8) -> Option<PathBuf> {
        if depth > 3 {
            return None;
        }
        let direct = dir.join(exe);
        if direct.is_file() {
            return Some(direct);
        }
        let entries = fs::read_dir(dir).ok()?;
        let mut nested_dirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some(exe) {
                return Some(path);
            }
            if path.is_dir() {
                nested_dirs.push(path);
            }
        }
        nested_dirs.iter().find_map(|sub| search(sub, exe, depth + 1))
    }
    search(dir, exe, 0)
}

/// Разобрать имя каталога сборки: `llama-b10688-ubuntu-vulkan-x64`
/// → ("b10688", Vulkan). Возвращает None для чужих каталогов.
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
    } else if rest.contains("rocm") {
        Backend::Rocm
    } else if rest.contains("sycl") {
        Backend::Sycl
    } else if rest.contains("openvino") {
        Backend::Openvino
    } else if rest.contains("opencl") {
        Backend::Opencl
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
    /// существующей сборки заменяет её содержимое. Для Windows CUDA
    /// вместе со сборкой ставится парный cudart-архив, если он есть.
    pub fn install(
        &self,
        asset: &BuildAsset,
        cancel: crate::download::CancelFlag,
        mut progress: impl FnMut(Progress),
    ) -> Result<InstalledBuild> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("не удалось создать каталог {}", self.dir.display()))?;

        let main_path = self.dir.join(format!(".download-{}", asset.asset.name));
        download_file(
            &asset.asset.browser_download_url,
            &main_path,
            cancel.clone(),
            |d, t| progress(Progress::Downloading { downloaded: d, total: t }),
        )
        .with_context(|| format!("скачивание {}", asset.asset.name))?;

        // cudart-рантайм нужен Windows CUDA сборкам (отдельный архив в релизе).
        let runtime_path = match &asset.runtime_asset {
            Some(runtime) => {
                if cancel.is_cancelled() {
                    return Err(anyhow::anyhow!("скачивание отменено"));
                }
                let path = self.dir.join(format!(".download-{}", runtime.name));
                download_file(&runtime.browser_download_url, &path, cancel.clone(), |_, _| {})
                    .with_context(|| format!("скачивание {}", runtime.name))?;
                Some(path)
            }
            None => None,
        };

        progress(Progress::Extracting);
        let result = install_archive(&main_path, runtime_path.as_deref(), self, asset);
        let _ = fs::remove_file(&main_path);
        if let Some(path) = runtime_path {
            let _ = fs::remove_file(path);
        }
        result
    }
}

/// Скачать файл по URL с отчётом о прогрессе (блокирующе).
/// Общая реализация — в `download.rs`; здесь без докачки и без заголовков.
pub fn download_file(
    url: &str,
    dest: &Path,
    cancel: crate::download::CancelFlag,
    progress: impl FnMut(u64, u64),
) -> Result<()> {
    crate::download::download_file(
        &crate::download::DownloadRequest {
            url: url.to_string(),
            dest: dest.to_path_buf(),
            headers: vec![("User-Agent".to_string(), USER_AGENT.to_string())],
            resume: false,
            cancel,
        },
        progress,
    )
    .with_context(|| format!("скачивание {url}"))
}

/// Распаковать скачанный архив релиза (zip или tar.gz) в каталог сборки
/// библиотеки. Устанавливает через staging-каталог: распаковка → замена
/// целевого каталога. `runtime_archive` (cudart для CUDA) распаковывается туда же.
fn install_archive(
    archive_path: &Path,
    runtime_archive: Option<&Path>,
    store: &BuildsStore,
    asset: &BuildAsset,
) -> Result<InstalledBuild> {
    let staging = store.dir.join(format!(".staging-{}", asset.dir_name()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .with_context(|| format!("не удалось создать каталог {}", staging.display()))?;

    let result = extract_archive(archive_path, &staging)
        .and_then(|()| match runtime_archive {
            Some(runtime) => extract_archive(runtime, &staging),
            None => Ok(()),
        })
        .and_then(|()| {
            if server_binary_in(&staging).is_none() {
                bail!("в архиве {} не найден llama-server", asset.asset.name);
            }
            Ok(())
        })
        .and_then(|()| {
            let final_dir = store.dir_for(asset);
            // Заменяем старую установку той же версии.
            let _ = fs::remove_dir_all(&final_dir);
            fs::rename(&staging, &final_dir).with_context(|| {
                format!("переименование {} → {}", staging.display(), final_dir.display())
            })
        });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }

    let final_dir = store.dir_for(asset);
    result?;
    let (tag, _) = parse_dir_name(&asset.dir_name())
        .with_context(|| format!("неожиданное имя каталога {}", asset.dir_name()))?;
    Ok(InstalledBuild {
        dir: final_dir,
        tag,
        backend: asset.backend,
    })
}

/// Распаковать zip или tar.gz в каталог (в зависимости от расширения файла).
fn extract_archive(archive_path: &Path, staging: &Path) -> Result<()> {
    let name = archive_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if name.ends_with(".tar.gz") {
        extract_targz(archive_path, staging)
    } else {
        extract_zip(archive_path, staging)
    }
}

fn extract_zip(archive_path: &Path, staging: &Path) -> Result<()> {
    let file = File::open(archive_path)
        .with_context(|| format!("не удалось открыть {}", archive_path.display()))?;
    let mut archive = ZipArchive::new(file).context("чтение zip-архива")?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).context("чтение записи zip")?;
        // enclosed_name отсекает zip-slip: пути, выходящие за каталог распаковки.
        let Some(relative) = entry.enclosed_name() else {
            continue;
        };
        unpack_entry(|out| std::io::copy(&mut entry, out), staging.join(relative))?;
    }
    Ok(())
}

fn extract_targz(archive_path: &Path, staging: &Path) -> Result<()> {
    let file = File::open(archive_path)
        .with_context(|| format!("не удалось открыть {}", archive_path.display()))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    // unpack сам проверяет пути на выход за каталог и сохраняет права
    // (исполнимый бит llama-server на Linux критичен для запуска).
    archive.unpack(staging).context("распаковка tar.gz")?;
    Ok(())
}

/// Записать файл архива по целевому пути, создав промежуточные каталоги.
fn unpack_entry(mut copy: impl FnMut(&mut File) -> std::io::Result<u64>, target: PathBuf) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("не удалось создать каталог {}", parent.display()))?;
    }
    let mut out = File::create(&target)
        .with_context(|| format!("не удалось создать файл {}", target.display()))?;
    copy(&mut out).with_context(|| format!("распаковка {}", target.display()))?;
    Ok(())
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
        let kind = crate::github::classify_asset(name).expect("имя должно классифицироваться");
        BuildAsset {
            asset: github::Asset {
                name: name.to_string(),
                browser_download_url: url.to_string(),
                size: 0,
            },
            tag: "b10688".into(),
            os: kind.os,
            arch: kind.arch,
            backend: kind.backend,
            runtime_asset: None,
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

    /// Собрать тестовый tar.gz с бинарником во вложенном каталоге
    /// (как в реальных релизах llama.cpp) с правом на исполнение.
    fn make_test_targz(path: &Path) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        let bin = b"#!/bin/sh\necho mock llama-server\n";
        let mut header = tar::Header::new_gnu();
        header.set_size(bin.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "llama-b10688-bin-ubuntu-x64/llama-server", &bin[..])
            .unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(11);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "llama-b10688-bin-ubuntu-x64/sub/libex.so", &b"binary-blob"[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }

    #[test]
    fn parse_dir_name_variants() {
        assert_eq!(
            parse_dir_name("llama-b10688-ubuntu-x64"),
            Some(("b10688".into(), Backend::Cpu))
        );
        assert_eq!(
            parse_dir_name("llama-b10688-win-cuda-12.4-x64"),
            Some(("b10688".into(), Backend::Cuda))
        );
        assert_eq!(
            parse_dir_name("llama-b10688-ubuntu-vulkan-x64"),
            Some(("b10688".into(), Backend::Vulkan))
        );
        assert_eq!(
            parse_dir_name("llama-b10688-ubuntu-rocm-7.14-x64"),
            Some(("b10688".into(), Backend::Rocm))
        );
        // Чужие каталоги и пустые теги не распознаются.
        assert_eq!(parse_dir_name("releases-cache.json"), None);
        assert_eq!(parse_dir_name("llama--ubuntu"), None);
    }

    #[test]
    fn install_zip_extracts_and_replaces() {
        let root = temp_root("install");
        let store = BuildsStore::new(root.clone());
        let asset = sample_asset("llama-b10688-bin-ubuntu-x64.zip", "https://unused");
        make_test_zip(&root.join("test.zip"));

        let build = install_archive(&root.join("test.zip"), None, &store, &asset).expect("установка");
        assert_eq!(build.tag, "b10688");
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
        install_archive(&root.join("test.zip"), None, &store, &asset).expect("повторная установка");
        assert!(build.server_binary().is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn install_targz_extracts_with_nested_dir_and_permissions() {
        let root = temp_root("install-targz");
        let store = BuildsStore::new(root.clone());
        let asset = sample_asset("llama-b10688-bin-ubuntu-vulkan-x64.tar.gz", "https://unused");
        assert_eq!(asset.backend, Backend::Vulkan);
        make_test_targz(&root.join("test.tar.gz"));

        let build = install_archive(&root.join("test.tar.gz"), None, &store, &asset)
            .expect("установка tar.gz");
        let bin = build.server_binary().expect("llama-server найден во вложенном каталоге");
        assert_eq!(fs::read(&bin).unwrap(), b"#!/bin/sh\necho mock llama-server\n");
        // Исполнимый бит сохранён (иначе llama-server не запустится).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&bin).unwrap().permissions().mode();
            assert_ne!(mode & 0o111, 0, "нет прав на исполнение у llama-server");
        }
        assert!(build.dir.join("llama-b10688-bin-ubuntu-x64/sub/libex.so").is_file());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn install_targz_with_runtime_archive_merges_into_one_dir() {
        let root = temp_root("install-runtime");
        let store = BuildsStore::new(root.clone());
        let asset = sample_asset("llama-b10688-bin-ubuntu-vulkan-x64.tar.gz", "https://unused");
        make_test_targz(&root.join("test.tar.gz"));
        make_test_zip(&root.join("runtime.zip"));

        let build = install_archive(
            &root.join("test.tar.gz"),
            Some(&root.join("runtime.zip")),
            &store,
            &asset,
        )
        .expect("установка с рантаймом");
        assert!(build.server_binary().is_some());
        assert!(build.dir.join("sub/libexample.so").is_file());

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

        let err = install_archive(&root.join("empty.zip"), None, &store, &asset);
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
            crate::download::CancelFlag::new(),
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
