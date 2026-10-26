use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

const LOCK_RETRY: Duration = Duration::from_millis(50);
const LOCK_TIMEOUT: Duration = Duration::from_secs(10);
const LOCK_STALE_AFTER: Duration = Duration::from_secs(60);

/// Replaces `path` with `bytes` by writing a temp file in the same directory and renaming it
/// over `path`, so readers see either the old or the new contents.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temp = temp_path(path)?;
    let result = write_and_sync(&temp, bytes).and_then(|()| fs::rename(&temp, path));
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Runs `f` while holding the mkdir lock `<path>.lock`. Retries every 50 ms for up to 10 s, and
/// removes a lock older than 60 s as left behind by a crashed holder.
pub fn with_lock<T>(path: &Path, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    let _guard = LockGuard::acquire(lock_path(path))?;
    f()
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

fn temp_path(path: &Path) -> io::Result<PathBuf> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("no file name in {}", path.display()),
        )
    })?;
    let mut temp = std::ffi::OsString::from(".");
    temp.push(name);
    temp.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    Ok(path.with_file_name(temp))
}

fn write_and_sync(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create_new(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

struct LockGuard {
    lock: PathBuf,
}

impl LockGuard {
    fn acquire(lock: PathBuf) -> io::Result<LockGuard> {
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match fs::create_dir(&lock) {
                Ok(()) => return Ok(LockGuard { lock }),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
                Err(err) => return Err(err),
            }
            if is_stale(&lock)? {
                match fs::remove_dir(&lock) {
                    Ok(()) => continue,
                    Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
                    Err(err) => return Err(err),
                }
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out waiting for lock {}", lock.display()),
                ));
            }
            thread::sleep(LOCK_RETRY);
        }
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.lock);
    }
}

fn is_stale(lock: &Path) -> io::Result<bool> {
    let modified = match fs::metadata(lock) {
        Ok(meta) => meta.modified()?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or(Duration::ZERO);
    Ok(age > LOCK_STALE_AFTER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, SystemTime};

    #[test]
    fn write_atomic_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");

        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
        let names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["status.json"]);
    }

    #[test]
    fn with_lock_serializes() {
        let dir = tempfile::tempdir().unwrap();
        let counter = Arc::new(dir.path().join("counter"));
        write_atomic(&counter, b"0").unwrap();

        let workers: Vec<_> = (0..2)
            .map(|_| {
                let counter = Arc::clone(&counter);
                thread::spawn(move || {
                    for _ in 0..100 {
                        with_lock(&counter, || {
                            let n: u32 = fs::read_to_string(&*counter)?.parse().unwrap();
                            // Widen the read-modify-write window so an unserialized pair would lose updates.
                            thread::yield_now();
                            write_atomic(&counter, (n + 1).to_string().as_bytes())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }

        assert_eq!(fs::read_to_string(&*counter).unwrap(), "200");
        assert!(!lock_path(&counter).exists());
    }

    #[test]
    fn stale_lock_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("status.json");
        let lock = lock_path(&path);
        fs::create_dir(&lock).unwrap();
        let old = SystemTime::now() - Duration::from_secs(120);
        fs::File::open(&lock).unwrap().set_modified(old).unwrap();

        let value = with_lock(&path, || Ok(42)).unwrap();

        assert_eq!(value, 42);
        assert!(!lock.exists());
    }
}
