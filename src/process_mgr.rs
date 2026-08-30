use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};

/// How often the monitor thread checks the child process and the port.
const MONITOR_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Time to wait after SIGTERM before forcing SIGKILL.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum entries kept from the server output for display.
const SERVER_LOG_CAPACITY: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServerState {
    Stopped,
    Starting,
    Ready,
    /// Crashed and auto-restart scheduled (waiting out the backoff).
    RestartScheduled,
    /// Crashed; auto-restart exhausted or disabled — user intervention needed.
    Crashed,
}

impl ServerState {
    pub fn label(self) -> &'static str {
        match self {
            ServerState::Stopped => "Остановлен",
            ServerState::Starting => "Запускается…",
            ServerState::Ready => "Готов",
            ServerState::RestartScheduled => "Аварийный перезапуск…",
            ServerState::Crashed => "Упал",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ServerConfig {
    pub binary: PathBuf,
    pub model: PathBuf,
    pub host: String,
    pub port: u16,
    pub extra_args: Vec<String>,
}

impl ServerConfig {
    /// The command line that will be executed, for preview and logging.
    pub fn command_line(&self) -> String {
        let quote = |s: &str| {
            shlex::try_quote(s)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| s.to_string())
        };
        let mut parts = vec![
            quote(self.binary.to_string_lossy().as_ref()),
            "--model".to_string(),
            quote(self.model.to_string_lossy().as_ref()),
            "--host".to_string(),
            self.host.clone(),
            "--port".to_string(),
            self.port.to_string(),
        ];
        parts.extend(self.extra_args.iter().map(|a| quote(a)));
        parts.join(" ")
    }
}

/// Collected output of the running server (stdout+stderr, timestamped lines).
#[derive(Default)]
pub struct ServerLog {
    lines: Vec<(chrono::DateTime<chrono::Local>, String)>,
}

impl ServerLog {
    fn push(&mut self, line: &str) {
        if self.lines.len() >= SERVER_LOG_CAPACITY {
            self.lines.remove(0);
        }
        self.lines.push((chrono::Local::now(), line.to_string()));
    }

    pub fn lines(&self) -> &[(chrono::DateTime<chrono::Local>, String)] {
        &self.lines
    }

    pub fn clear(&mut self) {
        self.lines.clear();
    }
}

struct Inner {
    child: Option<Child>,
    state: ServerState,
    user_stop: bool,
    stop_requested_at: Option<Instant>,
    /// Auto-restart attempts made inside the current window.
    restart_attempts: u32,
    window_started: Option<Instant>,
    /// Incremented on every spawn; lets stale threads (readers, monitor)
    /// detect that a newer process instance took over.
    generation: u64,
}

/// Everything the monitor thread needs, so it can fully manage the process
/// lifecycle including restarts.
struct Shared {
    inner: Mutex<Inner>,
    /// Same Arc as `ServerManager::server_log`; kept here for reader/monitor threads.
    server_log: Arc<Mutex<ServerLog>>,
    log_file: Mutex<Option<File>>,
    log_file_path: PathBuf,
    config: Mutex<Option<ServerConfig>>,
}

/// Manages a single llama-server child process: start/stop/restart, output
/// capture, health polling and automatic restart on crash.
pub struct ServerManager {
    shared: Arc<Shared>,
    server_log: Arc<Mutex<ServerLog>>,
    auto_restore: Arc<Mutex<crate::config::AutoRestore>>,
}

impl ServerManager {
    pub fn new(logs_dir: PathBuf, auto_restore: crate::config::AutoRestore) -> Self {
        let server_log = Arc::new(Mutex::new(ServerLog::default()));
        Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    child: None,
                    state: ServerState::Stopped,
                    user_stop: false,
                    stop_requested_at: None,
                    restart_attempts: 0,
                    window_started: None,
                    generation: 0,
                }),
                server_log: server_log.clone(),
                log_file: Mutex::new(None),
                log_file_path: logs_dir.join("server.log"),
                config: Mutex::new(None),
            }),
            server_log,
            auto_restore: Arc::new(Mutex::new(auto_restore)),
        }
    }

    /// Called when the user edits the auto-restore settings.
    pub fn set_auto_restore(&self, auto_restore: crate::config::AutoRestore) {
        if let Ok(mut guard) = self.auto_restore.lock() {
            *guard = auto_restore;
        }
    }

    pub fn state(&self) -> ServerState {
        self.shared
            .inner
            .lock()
            .map(|i| i.state)
            .unwrap_or(ServerState::Stopped)
    }

    pub fn server_log(&self) -> Arc<Mutex<ServerLog>> {
        self.server_log.clone()
    }

    /// Whether the server process is alive (any of the running states).
    pub fn is_running(&self) -> bool {
        matches!(
            self.state(),
            ServerState::Starting | ServerState::Ready | ServerState::RestartScheduled
        )
    }

    pub fn config(&self) -> Option<ServerConfig> {
        self.shared.config.lock().ok().and_then(|c| c.clone())
    }

    /// Spawn `llama-server` with the given configuration.
    pub fn start(&self, config: ServerConfig) -> Result<(), String> {
        validate_config(&config)?;
        let mut inner = self.shared.inner.lock().map_err(|_| "внутренняя блокировка")?;
        if matches!(
            inner.state,
            ServerState::Starting | ServerState::Ready | ServerState::RestartScheduled
        ) {
            return Err("Сервер уже запущен".to_string());
        }
        inner.user_stop = false;
        inner.stop_requested_at = None;
        inner.restart_attempts = 0;
        inner.window_started = None;
        spawn_locked(&self.shared, &mut inner, &config)?;
        let generation = inner.generation;
        *self.shared.config.lock().map_err(|_| "внутренняя блокировка")? = Some(config);
        spawn_monitor_thread(self.shared.clone(), self.auto_restore.clone(), generation);
        Ok(())
    }

    /// Graceful stop: SIGTERM, then SIGKILL after the timeout (handled by the
    /// monitor thread).
    pub fn stop(&self) {
        let mut inner = match self.shared.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return,
        };
        inner.user_stop = true;
        let pid = inner.child.as_mut().map(|child| child.id());
        if let Some(pid) = pid {
            info!("Остановка llama-server (PID {pid})…");
            inner.stop_requested_at = Some(Instant::now());
            if let Some(child) = inner.child.as_mut() {
                terminate(child);
            }
        } else if inner.state == ServerState::RestartScheduled {
            // No process (waiting out a restart backoff) — stop immediately.
            inner.state = ServerState::Stopped;
            info!("Аварийный перезапуск отменён");
        }
    }

    pub fn restart(&self) -> Result<(), String> {
        let Some(config) = self.config() else {
            return Err("Нет сохранённой конфигурации сервера".to_string());
        };
        if self.is_running() {
            self.stop();
            let deadline = Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT;
            while self.is_running() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        self.start(config)
    }
}

fn validate_config(config: &ServerConfig) -> Result<(), String> {
    if !config.binary.is_file() {
        return Err(format!(
            "Бинарник не найден: {}",
            config.binary.display()
        ));
    }
    if config.model.as_os_str().is_empty() {
        return Err("Не выбран файл модели".to_string());
    }
    if !config.model.is_file() {
        return Err(format!(
            "Файл модели не найден: {}",
            config.model.display()
        ));
    }
    if config.host.is_empty() {
        return Err("Не указан host".to_string());
    }
    Ok(())
}

/// Spawn the process and its reader threads. Caller must hold `inner`.
fn spawn_locked(shared: &Arc<Shared>, inner: &mut Inner, config: &ServerConfig) -> Result<(), String> {
    let mut child = Command::new(&config.binary)
        .arg("--model")
        .arg(&config.model)
        .arg("--host")
        .arg(&config.host)
        .arg("--port")
        .arg(config.port.to_string())
        .args(&config.extra_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("Не удалось запустить {}: {e}", config.binary.display()))?;

    inner.generation += 1;
    inner.state = ServerState::Starting;
    let generation = inner.generation;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    info!(
        "Запуск llama-server (PID {}): {}",
        child.id(),
        config.command_line()
    );
    inner.child = Some(child);

    // Fresh output journal per run.
    if let Ok(mut log) = shared.server_log.lock() {
        log.clear();
    }
    if let Ok(mut file) = shared.log_file.lock() {
        *file = None;
    }

    if let Some(stdout) = stdout {
        spawn_pipe_reader(stdout, shared, generation, "stdout");
    }
    if let Some(stderr) = stderr {
        spawn_pipe_reader(stderr, shared, generation, "stderr");
    }
    Ok(())
}

fn spawn_pipe_reader(
    pipe: impl std::io::Read + Send + 'static,
    shared: &Arc<Shared>,
    generation: u64,
    source: &'static str,
) {
    let shared = shared.clone();
    let _ = std::thread::Builder::new()
        .name(format!("llama-server-{source}"))
        .spawn(move || {
            let reader = BufReader::new(pipe);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if let Ok(mut log) = shared.server_log.lock() {
                            log.push(&line);
                        }
                        append_log_file(&shared.log_file, &shared.log_file_path, &line);
                    }
                    Err(_) => break,
                }
                // Stop reading when a newer process generation took over.
                let current = shared.inner.lock().map(|i| i.generation).unwrap_or(generation);
                if current != generation {
                    break;
                }
            }
            debug!("reader-поток ({source}) завершён");
        });
}

fn append_log_file(file: &Mutex<Option<File>>, path: &PathBuf, line: &str) {
    let mut guard = match file.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if guard.is_none() {
        *guard = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok();
    }
    if let Some(f) = guard.as_mut() {
        let stamped = format!("{} {}", chrono::Local::now().format("%H:%M:%S%.3f"), line);
        let _ = writeln!(f, "{stamped}");
    }
}

fn finish_log_file(file: &Mutex<Option<File>>) {
    if let Ok(mut guard) = file.lock() {
        // Drop the handle; the next run starts a fresh append.
        *guard = None;
    }
}

fn spawn_monitor_thread(
    shared: Arc<Shared>,
    auto_restore: Arc<Mutex<crate::config::AutoRestore>>,
    generation: u64,
) {
    let _ = std::thread::Builder::new()
        .name("llama-server-monitor".into())
        .spawn(move || monitor_loop(shared, auto_restore, generation));
}

fn monitor_loop(shared: Arc<Shared>, auto_restore: Arc<Mutex<crate::config::AutoRestore>>, mut generation: u64) {
    let mut ready = false;
    loop {
        std::thread::sleep(MONITOR_POLL_INTERVAL);

        let exit_status = {
            let mut inner = match shared.inner.lock() {
                Ok(inner) => inner,
                Err(_) => return,
            };
            if inner.generation != generation {
                // A newer instance took over; retire this monitor.
                return;
            }
            let Some(child) = inner.child.as_mut() else {
                return;
            };
            match child.try_wait() {
                Ok(Some(status)) => Some(status),
                Ok(None) => {
                    // Escalate to SIGKILL if SIGTERM was ignored for too long.
                    if let Some(requested_at) = inner.stop_requested_at {
                        if requested_at.elapsed() > GRACEFUL_SHUTDOWN_TIMEOUT {
                            warn!("Процесс не отреагировал на SIGTERM, применяется SIGKILL");
                            if let Some(child) = inner.child.as_mut() {
                                let _ = child.kill();
                            }
                            inner.stop_requested_at = None;
                        }
                    }
                    None
                }
                Err(e) => {
                    error!("Ошибка наблюдения за процессом: {e}");
                    inner.state = ServerState::Crashed;
                    return;
                }
            }
        };

        let Some(status) = exit_status else {
            // Still running: poll readiness via the port once.
            if !ready
                && let Some(config) = shared.config.lock().ok().and_then(|c| c.clone())
                && server_ready(&config.host, config.port)
            {
                ready = true;
                if let Ok(mut inner) = shared.inner.lock() {
                    if inner.generation == generation {
                        inner.state = ServerState::Ready;
                        info!(
                            "llama-server готов: http://{}:{}",
                            config.host, config.port
                        );
                    }
                }
            }
            continue;
        };

        // Process exited.
        let action = {
            let mut inner = match shared.inner.lock() {
                Ok(inner) => inner,
                Err(_) => return,
            };
            if inner.generation != generation {
                return;
            }
            inner.child = None;
            finish_log_file(&shared.log_file);
            if inner.user_stop {
                inner.state = ServerState::Stopped;
                inner.stop_requested_at = None;
                info!("llama-server остановлен ({status})");
                return;
            }
            error!("llama-server завершился аварийно: {status}");

            let policy = auto_restore.lock().map(|p| p.clone()).unwrap_or_default();
            let now = Instant::now();
            match inner.window_started {
                Some(started) if now.duration_since(started).as_secs() > policy.window_secs => {
                    inner.window_started = Some(now);
                    inner.restart_attempts = 0;
                }
                None => {
                    inner.window_started = Some(now);
                    inner.restart_attempts = 0;
                }
                _ => {}
            }
            if !policy.enabled || inner.restart_attempts >= policy.max_restarts {
                inner.state = ServerState::Crashed;
                if policy.enabled {
                    error!(
                        "Лимит автовосстановления исчерпан ({} попыток за окно {} сек). Требуется вмешательство пользователя.",
                        policy.max_restarts, policy.window_secs
                    );
                }
                return;
            }
            inner.restart_attempts += 1;
            let delay_secs = policy
                .backoff_start_secs
                .saturating_mul(1_u64 << (inner.restart_attempts - 1).min(10));
            warn!(
                "Аварийное завершение, попытка автовосстановления {}/{} через {delay_secs} сек…",
                inner.restart_attempts, policy.max_restarts
            );
            inner.state = ServerState::RestartScheduled;
            delay_secs
        };

        std::thread::sleep(Duration::from_secs(action));

        // Restart.
        let mut inner = match shared.inner.lock() {
            Ok(inner) => inner,
            Err(_) => return,
        };
        if inner.generation != generation || inner.user_stop {
            return;
        }
        let Some(config) = shared.config.lock().ok().and_then(|c| c.clone()) else {
            inner.state = ServerState::Crashed;
            return;
        };
        if let Err(e) = validate_config(&config) {
            error!("Перезапуск невозможен: {e}");
            inner.state = ServerState::Crashed;
            return;
        }
        if spawn_locked(&shared, &mut inner, &config).is_err() {
            inner.state = ServerState::Crashed;
            return;
        }
        generation += 1;
        ready = false;
        drop(inner);
    }
}

/// SIGTERM first; the monitor escalates to SIGKILL after the timeout.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // Signal by PID only; no pointers involved.
        let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
        if rc != 0 {
            warn!("SIGTERM для PID {pid} не доставлен (rc={rc}), применяется kill()");
            let _ = child.kill();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

fn port_responds(host: &str, port: u16) -> bool {
    use std::net::ToSocketAddrs;
    if host.is_empty() || port == 0 {
        return false;
    }
    let addr = format!("{host}:{port}");
    match addr.to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok(),
            None => false,
        },
        Err(_) => false,
    }
}

/// Full readiness probe: newer llama.cpp versions bind the HTTP port before
/// the model is loaded and answer `/health` with 503 until ready. A bare TCP
/// connect is therefore not enough. Logic:
/// - port does not respond → not ready;
/// - HTTP answer received → ready only if it is 200;
/// - port responds but says nothing HTTP-like → assume an old-style server
///   that only binds after loading → ready.
fn server_ready(host: &str, port: u16) -> bool {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    if !port_responds(host, port) {
        return false;
    }
    let addr = match format!("{host}:{port}").to_socket_addrs() {
        Ok(mut addrs) => match addrs.next() {
            Some(addr) => addr,
            None => return false,
        },
        Err(_) => return false,
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(1)) else {
        return false;
    };
    if stream.set_read_timeout(Some(Duration::from_secs(2))).is_err() {
        return false;
    }
    let request = format!("GET /health HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut buf = [0u8; 1024];
    match stream.read(&mut buf) {
        // No HTTP answer within timeout: an old-style server that binds late.
        Ok(0) | Err(_) => true,
        Ok(n) => String::from_utf8_lossy(&buf[..n]).contains(" 200 "),
    }
}

/// True if the given address can be bound (i.e. it is free).
#[allow(dead_code)] // used in unit tests
pub fn port_free(host: &str, port: u16) -> bool {
    TcpListener::bind((host, port)).is_ok()
}

/// True if something already accepts connections on the given address.
pub fn port_in_use(host: &str, port: u16) -> bool {
    port_responds(host, port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoRestore;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(name: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("llamacpp-mgr-{name}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let script = dir.join(name);
        std::fs::write(&script, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    /// Workaround for the ETXTBSY race when executing a file written in the
    /// same multithreaded process: retry briefly before giving up.
    fn start_with_retry(manager: &ServerManager, config: ServerConfig) {
        let mut last_err = None;
        for _ in 0..10 {
            match manager.start(config.clone()) {
                Ok(()) => return,
                Err(e) if e.contains("Text file busy") => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => panic!("start: {e}"),
            }
        }
        panic!("start: {:?} (после ретраев)", last_err);
    }

    #[test]
    fn port_free_works() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!port_free("127.0.0.1", port));
        drop(listener);
        // The kernel may linger on the closed socket for a moment.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !port_free("127.0.0.1", port) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(port_free("127.0.0.1", port));
    }

    #[test]
    fn start_stop_mock_server_reaches_ready() {
        // Our "server" is a script that sleeps; readiness is simulated by a
        // listener we own on the same port the monitor polls.
        let dir = temp_dir("ready");
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let manager = ServerManager::new(dir.clone(), AutoRestore::default());
        let script = write_script(&dir, "mock-ready.sh", "sleep 30\n");
        let config = ServerConfig {
            binary: script,
            model: dir.join("fake.gguf"),
            host: "127.0.0.1".into(),
            port,
            extra_args: vec![],
        };
        std::fs::write(&config.model, b"fake").unwrap();
        start_with_retry(&manager, config);
        assert_start_stop(&manager);
        drop(listener);
        std::fs::remove_dir_all(&dir).ok();
    }

    fn assert_start_stop(manager: &ServerManager) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while manager.state() != ServerState::Ready && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(manager.state(), ServerState::Ready);
        manager.stop();
        let deadline = Instant::now() + Duration::from_secs(5);
        while manager.state() != ServerState::Stopped && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(manager.state(), ServerState::Stopped);
    }

    #[test]
    fn output_is_captured_and_saved_to_file() {
        let dir = temp_dir("output");
        let script = write_script(&dir, "mock-server.sh", "echo READY_LINE\necho ERR_LINE >&2\nsleep 60\n");
        let manager = ServerManager::new(dir.clone(), AutoRestore::default());
        let config = ServerConfig {
            binary: script,
            model: dir.join("fake.gguf"),
            host: "127.0.0.1".into(),
            port: 1, // nothing listens; stays Starting — fine
            extra_args: vec![],
        };
        std::fs::write(&config.model, b"fake").unwrap();
        start_with_retry(&manager, config);

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let has_all = {
                let log = manager.server_log();
                let log = log.lock().unwrap();
                log.lines().iter().any(|(_, l)| l.contains("READY_LINE"))
                    && log.lines().iter().any(|(_, l)| l.contains("ERR_LINE"))
            };
            if has_all || Instant::now() > deadline {
                assert!(has_all, "ожидаемые строки не появились в журнале");
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let file_log = std::fs::read_to_string(dir.join("server.log")).unwrap_or_default();
        assert!(file_log.contains("READY_LINE") && file_log.contains("ERR_LINE"));

        manager.stop();
        let deadline = Instant::now() + Duration::from_secs(5);
        while manager.state() != ServerState::Stopped && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(manager.state(), ServerState::Stopped);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn crash_triggers_auto_restore_then_gives_up() {
        let dir = temp_dir("crash");
        let script = write_script(&dir, "mock-crash.sh", "exit 3\n");
        let auto_restore = AutoRestore {
            enabled: true,
            max_restarts: 2,
            window_secs: 60,
            backoff_start_secs: 1,
        };
        let manager = ServerManager::new(dir.clone(), auto_restore);
        let config = ServerConfig {
            binary: script,
            model: dir.join("fake.gguf"),
            host: "127.0.0.1".into(),
            port: 1,
            extra_args: vec![],
        };
        std::fs::write(&config.model, b"fake").unwrap();
        start_with_retry(&manager, config);

        // 2 restarts with 1s and 2s backoff → Crashed within ~8s.
        let deadline = Instant::now() + Duration::from_secs(12);
        while manager.state() != ServerState::Crashed && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
        }
        assert_eq!(manager.state(), ServerState::Crashed);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn disabled_auto_restore_leaves_crashed() {
        let dir = temp_dir("norestore");
        let script = write_script(&dir, "mock-crash2.sh", "exit 5\n");
        let auto_restore = AutoRestore {
            enabled: false,
            ..AutoRestore::default()
        };
        let manager = ServerManager::new(dir.clone(), auto_restore);
        let config = ServerConfig {
            binary: script,
            model: dir.join("fake.gguf"),
            host: "127.0.0.1".into(),
            port: 1,
            extra_args: vec![],
        };
        std::fs::write(&config.model, b"fake").unwrap();
        start_with_retry(&manager, config);

        let deadline = Instant::now() + Duration::from_secs(5);
        while manager.state() != ServerState::Crashed && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(manager.state(), ServerState::Crashed);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// End-to-end with a real llama-server. Run manually:
    /// `LLAMA_SERVER_BIN=/path/to/llama-server LLAMA_TEST_MODEL=/path/model.gguf \
    ///  cargo test real_server -- --ignored --nocapture`
    #[test]
    #[ignore = "requires a local llama-server binary and a GGUF model"]
    fn real_server_launch_ready_and_stop() {
        let Ok(binary) = std::env::var("LLAMA_SERVER_BIN") else { return };
        let Ok(model) = std::env::var("LLAMA_TEST_MODEL") else { return };
        let dir = temp_dir("real");
        // Pick a free port instead of a fixed one to avoid conflicts.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().unwrap().port()
        };
        let manager = ServerManager::new(dir.clone(), AutoRestore::default());
        let config = ServerConfig {
            binary: PathBuf::from(&binary),
            model: PathBuf::from(&model),
            host: "127.0.0.1".into(),
            port,
            // Minimal context; GPU offload mirrors a typical real setup.
            extra_args: vec!["-c".into(), "1024".into(), "-ngl".into(), "99".into()],
        };
        start_with_retry(&manager, config);

        let deadline = Instant::now() + Duration::from_secs(300);
        while manager.state() != ServerState::Ready && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(500));
        }
        assert_eq!(manager.state(), ServerState::Ready, "сервер не стал Ready за 300 сек");

        // /health must answer HTTP 200.
        let addr = format!("127.0.0.1:{port}");
        let mut stream = TcpStream::connect(addr).expect("connect");
        use std::io::Write as _;
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = String::new();
        use std::io::Read as _;
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("200"), "/health вернул не 200: {response}");

        manager.stop();
        let deadline = Instant::now() + Duration::from_secs(10);
        while manager.state() != ServerState::Stopped && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(manager.state(), ServerState::Stopped);
        println!("real launch OK: /health = 200, graceful stop OK");
        std::fs::remove_dir_all(&dir).ok();
    }
}
