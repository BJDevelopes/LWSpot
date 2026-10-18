//! Helpers shared by the unit tests.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// This test run's folder, created on first use.
static RUN: OnceLock<PathBuf> = OnceLock::new();

/// How old another run's folder must be before a new run removes it: a run
/// killed before its end could not remove its own, and no run takes this
/// long.
const ABANDONED: Duration = Duration::from_secs(60 * 60);

/// The folder tests write their files under, in place of the system's
/// temporary folder: `<temp>/lwspot-tests/<process id>`. The whole folder is
/// removed when the test run ends, so a test that does not tidy up, or
/// fails before it does, leaves nothing behind.
pub(crate) fn temp_dir() -> PathBuf {
    RUN.get_or_init(|| {
        let parent = std::env::temp_dir().join("lwspot-tests");
        remove_abandoned(&parent, ABANDONED);
        let run = parent.join(std::process::id().to_string());
        let _ = std::fs::create_dir_all(&run);
        remove_at_exit();
        run
    })
    .clone()
}

/// Removes the folders of earlier runs that ended without removing them:
/// those untouched for longer than `age`.
fn remove_abandoned(parent: &Path, age: Duration) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|untouched| untouched > age);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// Removes this run's folder when the test process exits. The test harness
/// returns from `main` once every test is done, and the C runtime then runs
/// the functions registered here.
fn remove_at_exit() {
    unsafe extern "C" {
        fn atexit(callback: extern "C" fn()) -> std::ffi::c_int;
    }
    extern "C" fn remove_run() {
        if let Some(run) = RUN.get() {
            let _ = std::fs::remove_dir_all(run);
        }
    }
    // SAFETY: `atexit` only stores the pointer to a function with no
    // arguments, which stays valid for the whole process.
    unsafe {
        atexit(remove_run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run's folder older than the limit goes; a recent one, which may be
    /// another run still going, stays.
    #[test]
    fn only_abandoned_run_folders_are_removed() {
        let parent = temp_dir().join("abandoned-runs");
        let run = parent.join("12345");
        std::fs::create_dir_all(run.join("config")).unwrap();
        remove_abandoned(&parent, ABANDONED);
        assert!(run.exists(), "a recent run is kept");
        std::thread::sleep(Duration::from_millis(30));
        remove_abandoned(&parent, Duration::from_millis(10));
        assert!(!run.exists(), "an old run is removed");
        assert!(temp_dir().exists(), "this run's own folder stays");
    }
}
