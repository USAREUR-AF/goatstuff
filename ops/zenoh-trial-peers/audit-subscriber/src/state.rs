//! Durable allocation prevents keys arriving behind a reader's StartAfter cursor.
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct Watermark {
    subscriber: String,
    last_ms: i64,
}

pub struct BatchClock {
    dir: PathBuf,
    state: Watermark,
    _lock: File,
}

impl BatchClock {
    pub fn open(dir: &Path, subscriber: &str, initialize_after: Option<i64>) -> Result<Self> {
        anyhow::ensure!(
            !subscriber.is_empty()
                && subscriber
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "subscriber ID must be a single alphanumeric, dash or underscore component"
        );
        std::fs::create_dir_all(dir).context("create batch state directory")?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join("writer.lock"))?;
        lock.try_lock()
            .context("another subscriber writer owns this state directory")?;
        let path = dir.join("watermark.json");
        if let Some(last_ms) = initialize_after {
            anyhow::ensure!(last_ms >= 0, "initial watermark must be nonnegative");
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .context("initialize state only once; preserve existing watermark")?;
            serde_json::to_writer(
                &mut f,
                &Watermark {
                    subscriber: subscriber.to_string(),
                    last_ms,
                },
            )?;
            f.write_all(b"\n")?;
            f.sync_all()?;
            File::open(dir)?.sync_all()?;
        }
        let state: Watermark = serde_json::from_reader(File::open(&path).context(
            "missing batch state: initialize above the last stored audit key before starting",
        )?)
        .context("invalid batch state; recover explicitly, never reset")?;
        anyhow::ensure!(
            state.subscriber == subscriber && state.last_ms >= 0,
            "batch state subscriber or watermark mismatch"
        );
        Ok(Self {
            dir: dir.to_path_buf(),
            state,
            _lock: lock,
        })
    }

    pub fn allocate(&mut self, now_ms: i64) -> Result<i64> {
        let next = now_ms.max(
            self.state
                .last_ms
                .checked_add(1)
                .context("batch clock exhausted")?,
        );
        let candidate = Watermark {
            subscriber: self.state.subscriber.clone(),
            last_ms: next,
        };
        let temp = self.dir.join("watermark.next");
        let mut f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp)?;
        serde_json::to_writer(&mut f, &candidate)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
        std::fs::rename(temp, self.dir.join("watermark.json"))?;
        File::open(&self.dir)?.sync_all()?;
        self.state = candidate;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn temp() -> PathBuf {
        std::env::temp_dir().join(format!(
            "goat-audit-clock-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ))
    }
    #[test]
    fn order_survives_same_millisecond_regression_restart_and_locking() {
        let dir = temp();
        assert!(BatchClock::open(&dir, "site", None).is_err());
        let mut c = BatchClock::open(&dir, "site", Some(100)).unwrap();
        assert_eq!(c.allocate(100).unwrap(), 101);
        assert_eq!(c.allocate(90).unwrap(), 102);
        assert!(BatchClock::open(&dir, "site", None).is_err());
        drop(c);
        let mut c = BatchClock::open(&dir, "site", None).unwrap();
        assert_eq!(c.allocate(99).unwrap(), 103);
        drop(c);
        assert!(BatchClock::open(&dir, "other", None).is_err());
        assert!(BatchClock::open(&dir, "site", Some(0)).is_err());
        std::fs::write(dir.join("watermark.json"), "broken").unwrap();
        assert!(BatchClock::open(&dir, "site", None).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
