use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Local};
use log::{Level, LevelFilter, Log, Metadata, Record};

/// Maximum number of log entries kept for display in the UI.
const UI_BUFFER_CAPACITY: usize = 2000;

#[derive(Clone, Debug)]
pub struct LogEntry {
    pub time: DateTime<Local>,
    pub level: Level,
    pub message: String,
}

#[derive(Default)]
pub struct LogBuffer {
    entries: Vec<LogEntry>,
}

impl LogBuffer {
    pub fn push(&mut self, entry: LogEntry) {
        if self.entries.len() >= UI_BUFFER_CAPACITY {
            self.entries.remove(0);
        }
        self.entries.push(entry);
    }

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

struct Logger {
    /// Current filter level as `u8` (0 = Off, 5 = Trace) so the UI can change it at runtime.
    filter: Arc<AtomicU8>,
    buffer: Arc<Mutex<LogBuffer>>,
    file: Mutex<Option<File>>,
    file_path: PathBuf,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.current_filter() && level_allowed(metadata.target(), metadata.level())
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let entry = LogEntry {
            time: Local::now(),
            level: record.level(),
            message: record.args().to_string(),
        };
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.push(entry.clone());
        }
        self.write_to_file(&entry);
    }

    fn flush(&self) {}
}

/// Keep the journal readable: our own messages at any level, third-party
/// libraries (e.g. the DBus stack behind the folder picker) only when
/// something is actually wrong.
fn level_allowed(target: &str, level: Level) -> bool {
    target.starts_with("llamacpp_manager") || level <= Level::Warn
}

impl Logger {
    fn current_filter(&self) -> LevelFilter {
        match self.filter.load(Ordering::Relaxed) {
            0 => LevelFilter::Off,
            1 => LevelFilter::Error,
            2 => LevelFilter::Warn,
            3 => LevelFilter::Info,
            4 => LevelFilter::Debug,
            _ => LevelFilter::Trace,
        }
    }

    fn write_to_file(&self, entry: &LogEntry) {
        let mut guard = match self.file.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        // Reopen the file if it went away (e.g. logs dir cleaned at runtime).
        if guard.is_none() {
            *guard = OpenOptions::new().create(true).append(true).open(&self.file_path).ok();
        }
        if let Some(file) = guard.as_mut() {
            let line = format!(
                "{} [{:<5}] {}\n",
                entry.time.format("%Y-%m-%d %H:%M:%S%.3f"),
                entry.level,
                entry.message
            );
            let _ = file.write_all(line.as_bytes());
        }
    }
}

/// Handle for changing the filter at runtime (the debug-logging toggle).
#[derive(Clone)]
pub struct LogHandle {
    filter: Arc<AtomicU8>,
    buffer: Arc<Mutex<LogBuffer>>,
}

impl LogHandle {
    pub fn set_debug(&self, enabled: bool) {
        self.filter.store(
            if enabled { LevelFilter::Debug as u8 } else { LevelFilter::Info as u8 },
            Ordering::Relaxed,
        );
        log::set_max_level(self.filter_level());
    }

    fn filter_level(&self) -> LevelFilter {
        match self.filter.load(Ordering::Relaxed) {
            4 => LevelFilter::Debug,
            _ => LevelFilter::Info,
        }
    }

    pub fn snapshot(&self) -> Vec<LogEntry> {
        self.buffer.lock().map(|b| b.entries().to_vec()).unwrap_or_default()
    }

    pub fn clear(&self) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
    }
}

/// Install the global logger. Safe to call once; failures are reported via
/// the returned message instead of panicking.
pub fn init(debug_logging: bool, logs_dir: &Path) -> (Option<LogHandle>, Option<String>) {
    let filter = Arc::new(AtomicU8::new(if debug_logging { 4 } else { 3 }));
    let buffer = Arc::new(Mutex::new(LogBuffer::default()));
    let file_path = logs_dir.join("manager.log");
    let logger = Logger {
        filter: filter.clone(),
        buffer: buffer.clone(),
        file: Mutex::new(OpenOptions::new().create(true).append(true).open(&file_path).ok()),
        file_path: file_path.clone(),
    };

    match log::set_boxed_logger(Box::new(logger)) {
        Ok(()) => {
            log::set_max_level(if debug_logging { LevelFilter::Debug } else { LevelFilter::Info });
            let handle = LogHandle { filter, buffer };
            (Some(handle), None)
        }
        Err(e) => (None, Some(format!("логгер уже инициализирован: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_trims_to_capacity() {
        let mut buffer = LogBuffer::default();
        for i in 0..(UI_BUFFER_CAPACITY + 100) {
            buffer.push(LogEntry {
                time: Local::now(),
                level: Level::Info,
                message: format!("entry {i}"),
            });
        }
        assert_eq!(buffer.entries().len(), UI_BUFFER_CAPACITY);
        assert_eq!(buffer.entries().last().unwrap().message, format!("entry {}", UI_BUFFER_CAPACITY + 99));
    }

    #[test]
    fn debug_toggle_updates_buffer_content() {
        let dir = std::env::temp_dir().join(format!("llamacpp-manager-log-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (handle, err) = init(false, &dir);
        let handle = handle.expect("logger must init in tests");
        assert!(err.is_none());

        log::debug!("debug message");
        log::info!("info message");
        assert!(handle.snapshot().iter().all(|e| e.message != "debug message"));
        assert!(handle.snapshot().iter().any(|e| e.message == "info message"));

        handle.set_debug(true);
        log::debug!("debug message after toggle");
        assert!(handle.snapshot().iter().any(|e| e.message == "debug message after toggle"));

        handle.clear();
        assert!(handle.snapshot().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
