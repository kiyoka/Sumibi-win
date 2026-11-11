//! Opt-in local conversion diagnostics shared by the settings UI and TIP.
//! The TIP queues records on the STA and performs file I/O on its worker.
use std::ffi::{c_void, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows::core::{GUID, HRESULT, PWSTR};

// FOLDERID_LocalAppData. Resolve through the Shell so the path does not
// depend on the environment inherited by the application hosting the TIP.
const LOCAL_APP_DATA: GUID = GUID::from_u128(0xf1b32785_6fba_4fcf_9d55_7b8e7f157091);

#[link(name = "shell32")]
extern "system" {
    fn SHGetKnownFolderPath(
        id: *const GUID,
        flags: u32,
        token: *mut c_void,
        path: *mut PWSTR,
    ) -> HRESULT;
}
#[link(name = "ole32")]
extern "system" {
    fn CoTaskMemFree(memory: *mut c_void);
}

pub fn directory() -> io::Result<PathBuf> {
    let mut buffer = PWSTR::null();
    // The Shell returns a task-allocated string, including on some failures.
    let status =
        unsafe { SHGetKnownFolderPath(&LOCAL_APP_DATA, 0, std::ptr::null_mut(), &mut buffer) };
    let local = (!buffer.is_null() && status.is_ok())
        .then(|| PathBuf::from(unsafe { OsString::from_wide(buffer.as_wide()) }));
    unsafe { CoTaskMemFree(buffer.0.cast()) };
    local
        .filter(|path| path.is_absolute())
        .map(|path| path.join("Sumibi").join("conversion-logs"))
        .ok_or_else(|| io::Error::other("LocalAppData unavailable"))
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn generation(directory: &std::path::Path) -> u64 {
    let mut bytes = [0; 8];
    File::open(directory.join("enabled"))
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_or(0, |_| u64::from_le_bytes(bytes))
}

pub fn enabled() -> bool {
    directory().is_ok_and(|path| generation(&path) != 0)
}

pub fn set_enabled(enable: bool) -> io::Result<()> {
    let path = directory()?;
    let marker = path.join("enabled");
    let result = if enable {
        fs::create_dir_all(path)?;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(marker)
            .and_then(|mut file| {
                let generation = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64;
                file.write_all(&generation.max(1).to_le_bytes())
            })
    } else {
        fs::remove_file(marker)
    };
    match result {
        Err(error)
            if (enable && error.kind() == io::ErrorKind::AlreadyExists)
                || (!enable && error.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(())
        }
        result => result,
    }
}

struct Entry {
    generation: u64,
    time_ms: u64,
    request: Option<u64>,
    event: &'static str,
    detail: String,
    input: Option<String>,
}

pub struct Recorder {
    mode: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    sender: SyncSender<Entry>,
}

impl Recorder {
    pub fn record(
        &self,
        request: Option<u64>,
        event: &'static str,
        detail: fmt::Arguments<'_>,
        input: Option<&str>,
    ) {
        let generation = self.mode.load(Ordering::Relaxed);
        if generation == 0 {
            return;
        }
        if self
            .sender
            .try_send(Entry {
                generation,
                time_ms: timestamp_ms(),
                request,
                event,
                detail: detail.to_string(),
                input: input.map(str::to_owned),
            })
            .is_err()
        {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub struct Writer {
    mode: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    receiver: Receiver<Entry>,
    directory: Option<PathBuf>,
    next_check: Instant,
    file: Option<File>,
    app: Option<OsString>,
}

pub fn channel() -> (Recorder, Writer) {
    let mode = Arc::new(AtomicU64::new(0));
    let dropped = Arc::new(AtomicU64::new(0));
    let (sender, receiver) = mpsc::sync_channel(128);
    (
        Recorder {
            mode: mode.clone(),
            dropped: dropped.clone(),
            sender,
        },
        Writer {
            mode,
            dropped,
            receiver,
            directory: None,
            next_check: Instant::now(),
            file: None,
            app: None,
        },
    )
}

impl Writer {
    pub fn poll(&mut self) {
        let now = Instant::now();
        if now < self.next_check {
            return;
        }
        self.next_check = now + Duration::from_millis(250);
        let directory = self
            .directory
            .get_or_insert_with(|| directory().unwrap_or_default());
        let active = if directory.is_absolute() {
            generation(directory)
        } else {
            0
        };
        if active != self.mode.swap(active, Ordering::Relaxed) {
            self.file = None;
            self.dropped.store(0, Ordering::Relaxed);
        }
        for entry in self.receiver.try_iter().take(128) {
            if active == 0 || entry.generation != active {
                continue;
            }
            let pid = std::process::id();
            if self.file.is_none() {
                static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
                let sequence = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
                let name = format!("{}-{pid}-{sequence}.jsonl", timestamp_ms());
                self.file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join(name))
                    .ok();
            }
            let Some(file) = &mut self.file else {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let app = self.app.get_or_insert_with(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|path| path.file_name().map(OsString::from))
                    .unwrap_or_default()
            });
            let dropped = self.dropped.swap(0, Ordering::Relaxed);
            let record = serde_json::json!({
                "time_unix_ms": entry.time_ms, "pid": pid, "app": app.to_string_lossy(),
                "request": entry.request, "event": entry.event, "detail": entry.detail,
                "input": entry.input, "dropped_records": dropped,
            });
            let mut line = record.to_string();
            line.push('\n');
            if file.write_all(line.as_bytes()).is_err() {
                self.dropped
                    .fetch_add(dropped.saturating_add(1), Ordering::Relaxed);
                self.file = None;
            }
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        self.next_check = Instant::now();
        self.poll();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_only_enabled_generation_and_preserves_utf8_input() {
        let directory = std::env::temp_dir().join(format!(
            "sumibi-conversion-log-{}-{}",
            std::process::id(),
            timestamp_ms()
        ));
        fs::create_dir(&directory).unwrap();
        let marker = directory.join("enabled");
        let (recorder, mut writer) = channel();
        writer.directory = Some(directory.clone());

        recorder.record(
            Some(1),
            "input_verified",
            format_args!("off"),
            Some("hidden"),
        );
        fs::write(&marker, 1_u64.to_le_bytes()).unwrap();
        writer.poll();
        recorder.record(
            Some(2),
            "input_verified",
            format_args!("on"),
            Some("日本語\n入力"),
        );
        writer.next_check = Instant::now();
        writer.poll();

        recorder.record(
            Some(3),
            "input_verified",
            format_args!("previous mode"),
            Some("hidden"),
        );
        fs::write(&marker, 2_u64.to_le_bytes()).unwrap();
        writer.next_check = Instant::now();
        writer.poll();
        drop(writer);

        let files: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(Result::unwrap)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        assert_eq!(files.len(), 1);
        let contents = fs::read_to_string(files[0].path()).unwrap();
        let entry: serde_json::Value = serde_json::from_str(&contents).unwrap();
        assert_eq!(entry["request"], 2);
        assert_eq!(entry["input"], "日本語\n入力");
        fs::remove_file(files[0].path()).unwrap();
        fs::remove_file(marker).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
