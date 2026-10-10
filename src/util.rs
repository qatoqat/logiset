use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if Path::new(&v).is_absolute() => PathBuf::from(v),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/tmp".into())).join(fallback),
    }
}

/// `$XDG_CONFIG_HOME/logiset`
pub fn config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("logiset")
}

/// `$XDG_DATA_HOME/logiset`
pub fn data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("logiset")
}

/// Directory for onboard-memory backups taken before every write.
pub fn backup_dir() -> PathBuf {
    data_dir().join("backups")
}

/// Write via a synced temporary file and rename, then sync the directory, so the file is
/// complete and on disk when this returns (backups are taken before writing device memory).
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp)?;
    file.write_all(data)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;
    fs::File::open(dir)?.sync_all()
}

/// Seconds since the Unix epoch, for file names.
pub fn timestamp() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn process_running(name: &str) -> bool {
    let Ok(dir) = fs::read_dir("/proc") else {
        return false;
    };
    dir.flatten()
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .any(|e| fs::read_to_string(e.path().join("comm")).map(|c| c.trim() == name).unwrap_or(false))
}

/// Whether keyledsd (which drives keyboard LEDs continuously) is running. Cached for a few seconds.
pub fn keyledsd_running() -> bool {
    static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    match *cache {
        Some((at, running)) if at.elapsed() < Duration::from_secs(5) => running,
        _ => {
            let running = process_running("keyledsd");
            *cache = Some((Instant::now(), running));
            running
        }
    }
}
