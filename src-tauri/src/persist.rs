use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

static SAVE_LOCK: Mutex<()> = Mutex::new(());
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("state"))
        .to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

pub fn load_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    read_json(path).or_else(|| read_json(&sibling_with_suffix(path, ".bak")))
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let _guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("state path has no parent directory"))?;
    std::fs::create_dir_all(parent)?;

    let data = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    let suffix = format!(
        ".tmp-{}-{}",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    );
    let temp = sibling_with_suffix(path, &suffix);
    let backup = sibling_with_suffix(path, ".bak");

    let write_result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(&data)?;
        file.sync_all()
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    let had_primary = path.exists();
    if had_primary {
        if backup.exists() {
            if let Err(e) = std::fs::remove_file(&backup) {
                let _ = std::fs::remove_file(&temp);
                return Err(e);
            }
        }
        if let Err(e) = std::fs::rename(path, &backup) {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    }

    if let Err(e) = std::fs::rename(&temp, path) {
        if had_primary {
            let _ = std::fs::rename(&backup, path);
        }
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Payload {
        value: u32,
    }

    fn test_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "whatrust_persist_test_{}_{}",
            std::process::id(),
            tag
        ))
    }

    #[test]
    fn interrupted_primary_recovers_from_backup() {
        let dir = test_dir("recovery");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");

        super::save_json(&path, &Payload { value: 1 }).unwrap();
        super::save_json(&path, &Payload { value: 2 }).unwrap();
        std::fs::write(&path, b"{").unwrap();

        assert_eq!(
            super::load_json(&path),
            Some(Payload { value: 1 }),
            "a corrupt primary must not discard the last recoverable state"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_replaces_primary_and_keeps_previous_backup() {
        let dir = test_dir("replace");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");

        super::save_json(&path, &Payload { value: 7 }).unwrap();
        super::save_json(&path, &Payload { value: 8 }).unwrap();

        assert_eq!(super::load_json(&path), Some(Payload { value: 8 }));
        assert_eq!(
            super::load_json(&dir.join("state.json.bak")),
            Some(Payload { value: 7 })
        );
        assert!(
            std::fs::read_dir(&dir).unwrap().all(|e| !e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".tmp-")),
            "a successful save must not leave temporary files"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
