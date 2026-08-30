//! Общее потоковое скачивание файлов по HTTP: прогресс, докачка (Range), отмена.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

const USER_AGENT: &str = "llamacpp-manager";
const CHUNK: usize = 64 * 1024;
/// Промежуточные отчёты прогресса не чаще раза в мегабайт.
const PROGRESS_STEP: u64 = 1024 * 1024;

/// Общий флаг отмены для фонового скачивания.
#[derive(Clone, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Параметры скачивания файла.
pub struct DownloadRequest {
    pub url: String,
    pub dest: PathBuf,
    /// Дополнительные заголовки (например, Authorization для HuggingFace).
    pub headers: Vec<(String, String)>,
    /// Докачивать частично скачанный файл через Range-запрос.
    pub resume: bool,
    /// Флаг отмены: частичный файл сохраняется для последующей докачки.
    pub cancel: CancelFlag,
}

/// Размер файла на сервере через HEAD-запрос (для проверки докачки).
fn head_content_length(url: &str, headers: &[(String, String)]) -> Result<u64> {
    let mut req = ureq::head(url).header("User-Agent", USER_AGENT);
    for (key, value) in headers {
        req = req.header(key, value);
    }
    let response = req
        .config()
        .timeout_connect(Some(Duration::from_secs(30)))
        .build()
        .call()
        .context("HEAD-запрос")?;
    response
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .context("в HEAD-ответе нет Content-Length")
}

/// Скачать файл с отчётом о прогрессе (блокирующе, для фонового потока).
pub fn download_file(request: &DownloadRequest, mut progress: impl FnMut(u64, u64)) -> Result<()> {
    let mut start = if request.resume {
        fs::metadata(&request.dest).map(|m| m.len()).unwrap_or(0)
    } else {
        0
    };

    let build_request = || {
        let mut req = ureq::get(&request.url)
            .header("User-Agent", USER_AGENT)
            .header("Accept", "*/*");
        for (key, value) in &request.headers {
            req = req.header(key, value);
        }
        if start > 0 {
            req = req.header("Range", &format!("bytes={start}-"));
        }
        req.config()
            .timeout_connect(Some(Duration::from_secs(30)))
            .build()
    };

    let result = build_request().call();

    // 416 (Range Not Satisfiable): файл уже скачан полностью или частичный
    // файл битый. Сверяем размер с сервером (HEAD) и решаем: готово или заново.
    if start > 0
        && let Err(error) = &result
        && error.to_string().contains("http status: 416")
    {
        let remote_size = head_content_length(&request.url, &request.headers)?;
        if remote_size == start {
            progress(start, start);
            return Ok(());
        }
        let _ = fs::remove_file(&request.dest);
        return download_file(
            &DownloadRequest {
                url: request.url.clone(),
                dest: request.dest.clone(),
                headers: request.headers.clone(),
                resume: false,
                cancel: request.cancel.clone(),
            },
            progress,
        );
    }

    let mut response = result.with_context(|| format!("HTTP-запрос {}", request.url))?;

    // При resume сервер мог проигнорировать Range (ответ 200) — качаем заново.
    if start > 0 && response.status() != 206 {
        start = 0;
    }

    let content_length = response
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let total = if start > 0 {
        start + content_length
    } else {
        content_length
    };

    let mut reader = response.body_mut().as_reader();
    let mut file = if start > 0 {
        OpenOptions::new()
            .append(true)
            .open(&request.dest)
            .with_context(|| format!("открытие {}", request.dest.display()))?
    } else {
        if let Some(parent) = request.dest.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("не удалось создать каталог {}", parent.display())
            })?;
        }
        File::create(&request.dest)
            .with_context(|| format!("создание {}", request.dest.display()))?
    };

    let mut downloaded = start;
    let mut last_reported = start;
    let mut chunk = [0u8; CHUNK];
    loop {
        if request.cancel.is_cancelled() {
            // Частичный файл оставляем на диске — resume докачает с этого места.
            return Err(anyhow::anyhow!("скачивание отменено"));
        }
        let read = reader.read(&mut chunk).context("чтение тела ответа")?;
        if read == 0 {
            break;
        }
        file.write_all(&chunk[..read]).context("запись файла")?;
        downloaded += read as u64;
        if downloaded - last_reported >= PROGRESS_STEP {
            last_reported = downloaded;
            progress(downloaded, total);
        }
    }
    file.flush().context("запись файла")?;
    progress(downloaded, if total > 0 { total } else { downloaded });
    Ok(())
}
