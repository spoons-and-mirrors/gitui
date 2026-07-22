use std::{
    env,
    fs::{self, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{SyncSender, TrySendError, sync_channel},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const LOG_LIMIT_BYTES: u64 = 4 * 1024 * 1024;
const SLOW_ACTIVITY: Duration = Duration::from_millis(100);
const STALLED_ACTIVITY: Duration = Duration::from_secs(2);

static DIAGNOSTICS: OnceLock<Diagnostics> = OnceLock::new();
static DROP_REAPER: OnceLock<SyncSender<DropJob>> = OnceLock::new();
static NEXT_ACTIVITY_ID: AtomicU64 = AtomicU64::new(1);

type DropJob = Box<dyn FnOnce() + Send>;

struct Diagnostics {
    sender: SyncSender<String>,
    path: PathBuf,
    activities: Arc<Mutex<Vec<ActiveActivity>>>,
    main_thread: thread::ThreadId,
}

struct ActiveActivity {
    id: u64,
    phase: &'static str,
    detail: String,
    started: Instant,
}

pub(crate) struct Activity {
    id: Option<u64>,
    phase: &'static str,
    detail: String,
    started: Instant,
}

pub(crate) fn init() -> io::Result<PathBuf> {
    if let Some(diagnostics) = DIAGNOSTICS.get() {
        return Ok(diagnostics.path.clone());
    }
    let path = log_path()?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    rotate_if_needed(&path)?;
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let (sender, receiver) = sync_channel::<String>(4096);
    let activities = Arc::new(Mutex::new(Vec::new()));
    let diagnostics = Diagnostics {
        sender,
        path: path.clone(),
        activities: Arc::clone(&activities),
        main_thread: thread::current().id(),
    };
    if DIAGNOSTICS.set(diagnostics).is_err() {
        return Ok(path);
    }
    thread::Builder::new()
        .name("hunkle-log".to_owned())
        .spawn(move || {
            let mut writer = BufWriter::new(file);
            while let Ok(line) = receiver.recv() {
                let _ = writeln!(writer, "{line}");
                let _ = writer.flush();
            }
        })?;
    thread::Builder::new()
        .name("hunkle-watchdog".to_owned())
        .spawn(move || watchdog(activities))?;
    Ok(path)
}

pub(crate) fn event(message: impl Into<String>) {
    send("INFO", message.into());
}

pub(crate) fn activity(phase: &'static str, detail: impl Into<String>) -> Activity {
    let detail = detail.into();
    let started = Instant::now();
    let id = DIAGNOSTICS.get().and_then(|diagnostics| {
        if thread::current().id() != diagnostics.main_thread {
            return None;
        }
        let id = NEXT_ACTIVITY_ID.fetch_add(1, Ordering::Relaxed);
        diagnostics.activities.lock().ok()?.push(ActiveActivity {
            id,
            phase,
            detail: detail.clone(),
            started,
        });
        Some(id)
    });
    Activity {
        id,
        phase,
        detail,
        started,
    }
}

pub(crate) fn panic(message: String) {
    let Some(diagnostics) = DIAGNOSTICS.get() else {
        return;
    };
    let line = line("PANIC", message);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&diagnostics.path)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

pub(crate) fn drop_in_background<T>(label: &'static str, value: T)
where
    T: Send + 'static,
{
    let job: DropJob = Box::new(move || {
        let started = Instant::now();
        drop(value);
        let elapsed = started.elapsed();
        if elapsed >= SLOW_ACTIVITY {
            event(format!(
                "background drop label={label} elapsed_ms={}",
                elapsed.as_millis()
            ));
        }
    });
    match drop_reaper().try_send(job) {
        Ok(()) => {}
        Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => job(),
    }
}

fn drop_reaper() -> &'static SyncSender<DropJob> {
    DROP_REAPER.get_or_init(|| {
        let (sender, receiver) = sync_channel::<DropJob>(2);
        let _ = thread::Builder::new()
            .name("hunkle-drop".to_owned())
            .spawn(move || {
                while let Ok(job) = receiver.recv() {
                    job();
                }
            });
        sender
    })
}

impl Drop for Activity {
    fn drop(&mut self) {
        let elapsed = self.started.elapsed();
        if let Some(diagnostics) = DIAGNOSTICS.get()
            && let Some(id) = self.id
            && let Ok(mut activities) = diagnostics.activities.lock()
            && let Some(index) = activities.iter().position(|activity| activity.id == id)
        {
            activities.remove(index);
        }
        if elapsed >= SLOW_ACTIVITY {
            send(
                "WARN",
                format!(
                    "slow phase={} elapsed_ms={} {}",
                    self.phase,
                    elapsed.as_millis(),
                    self.detail
                ),
            );
        }
    }
}

fn watchdog(activities: Arc<Mutex<Vec<ActiveActivity>>>) {
    let mut last_report = None;
    loop {
        thread::sleep(Duration::from_secs(1));
        let report = activities.lock().ok().and_then(|activities| {
            let activity = activities.last()?;
            let elapsed = activity.started.elapsed();
            (elapsed >= STALLED_ACTIVITY).then(|| {
                (
                    activity.id,
                    elapsed.as_secs(),
                    activity.phase,
                    activity.detail.clone(),
                    elapsed.as_millis(),
                )
            })
        });
        let Some((id, second, phase, detail, elapsed_ms)) = report else {
            last_report = None;
            continue;
        };
        if last_report == Some((id, second)) {
            continue;
        }
        last_report = Some((id, second));
        send(
            "ERROR",
            format!("stalled phase={phase} elapsed_ms={elapsed_ms} {detail}"),
        );
    }
}

fn send(level: &str, message: String) {
    if let Some(diagnostics) = DIAGNOSTICS.get() {
        let _ = diagnostics.sender.try_send(line(level, message));
    }
}

fn line(level: &str, message: String) -> String {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}.{:03} {level} {message}",
        elapsed.as_secs(),
        elapsed.subsec_millis()
    )
}

fn log_path() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("HUNKLE_LOG") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("hunkle").join("hunkle.log"));
    }
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory is unavailable"))?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("hunkle")
        .join("hunkle.log"))
}

fn rotate_if_needed(path: &Path) -> io::Result<()> {
    if path
        .metadata()
        .is_ok_and(|metadata| metadata.len() >= LOG_LIMIT_BYTES)
    {
        let rotated = path.with_extension("log.old");
        let _ = fs::remove_file(&rotated);
        fs::rename(path, rotated)?;
    }
    Ok(())
}
