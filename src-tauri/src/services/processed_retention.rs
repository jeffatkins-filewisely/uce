//! Delete old files under `C:\FileWisely\Processed` and `C:\FileWisely\Failed` so uploads do not fill the disk.
//!
//! - **`UCE_PROCESSED_RETENTION_DAYS`** — e.g. `90` for Processed.
//! - **`UCE_FAILED_RETENTION_DAYS`** — optional; if unset, Failed uses the same days as Processed when that is set.
//! - Unset or `0` for both (and no Failed override) = no sweeper thread.

#[cfg(windows)]
mod imp {
    use crate::config::print_config;
    use std::fs;
    use std::path::Path;
    use std::thread;
    use std::time::{Duration, SystemTime};

    const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 3600);
    const STARTUP_DELAY: Duration = Duration::from_secs(120);

    fn days_to_duration(days: u64) -> Duration {
        Duration::from_secs(days.saturating_mul(24 * 3600))
    }

    fn sweep_dir(label: &str, dir: &Path, retention: Duration) -> (usize, usize) {
        let now = SystemTime::now();
        let mut removed = 0usize;
        let mut errors = 0usize;
        let Ok(read) = fs::read_dir(dir) else {
            return (0, 0);
        };
        for ent in read.flatten() {
            let path = ent.path();
            if !path.is_file() {
                continue;
            }
            let Ok(meta) = fs::metadata(&path) else {
                errors += 1;
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let age = now.duration_since(modified).unwrap_or_default();
            if age < retention {
                continue;
            }
            match fs::remove_file(&path) {
                Ok(()) => {
                    removed += 1;
                    eprintln!(
                        "[UCE][filewisely_retention] {} removed age>{:?} path={}",
                        label,
                        retention,
                        path.display()
                    );
                }
                Err(e) => {
                    errors += 1;
                    eprintln!(
                        "[UCE][filewisely_retention] {} remove failed path={} err={}",
                        label,
                        path.display(),
                        e
                    );
                }
            }
        }
        (removed, errors)
    }

    pub fn spawn() {
        let processed_days = print_config::uce_processed_retention_days();
        let failed_days = print_config::uce_failed_retention_days().or(processed_days);

        if processed_days.is_none() && failed_days.is_none() {
            return;
        }

        let dir_p = print_config::filewisely_processed_dir();
        let dir_f = print_config::filewisely_failed_dir();

        eprintln!(
            "[UCE][filewisely_retention] enabled: Processed={:?} days → {:?}, Failed={:?} days → {:?} (sweep every {:?})",
            processed_days,
            dir_p.display(),
            failed_days,
            dir_f.display(),
            SWEEP_INTERVAL
        );

        thread::spawn(move || {
            thread::sleep(STARTUP_DELAY);
            loop {
                let mut total_r = 0usize;
                let mut total_e = 0usize;

                if let Some(days) = processed_days {
                    let dur = days_to_duration(days);
                    let _ = fs::create_dir_all(&dir_p);
                    let (n, err) = sweep_dir("processed", &dir_p, dur);
                    total_r += n;
                    total_e += err;
                    if n > 0 || err > 0 {
                        eprintln!(
                            "[UCE][filewisely_retention] processed sweep: removed={} errors={} (retention {} days)",
                            n, err, days
                        );
                    }
                }

                if let Some(days) = failed_days {
                    let dur = days_to_duration(days);
                    let _ = fs::create_dir_all(&dir_f);
                    let (n, err) = sweep_dir("failed", &dir_f, dur);
                    total_r += n;
                    total_e += err;
                    if n > 0 || err > 0 {
                        eprintln!(
                            "[UCE][filewisely_retention] failed sweep: removed={} errors={} (retention {} days)",
                            n, err, days
                        );
                    }
                }

                if total_r > 0 || total_e > 0 {
                    eprintln!(
                        "[UCE][filewisely_retention] sweep cycle total removed={} errors={}",
                        total_r, total_e
                    );
                }

                thread::sleep(SWEEP_INTERVAL);
            }
        });
    }
}

#[cfg(windows)]
pub use imp::spawn;

#[cfg(not(windows))]
pub fn spawn() {}
