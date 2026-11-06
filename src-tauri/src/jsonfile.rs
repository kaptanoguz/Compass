//! Safe JSON reading and writing for state files. A write goes to a temporary
//! file in the same folder first, is forced to disk (fsync) and then moved into
//! place in one step, so a crash or power cut never leaves a half-written file.
//! The previous good copy is kept as `.bak`.

use serde::{de::DeserializeOwned, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn read_value(file: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(file).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_json_atomic<T: Serialize + ?Sized>(file: &Path, data: &T) -> std::io::Result<()> {
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let base = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{}.{}.{}.tmp", base, std::process::id(), now_ms()));
    let body = serde_json::to_vec_pretty(data).map_err(std::io::Error::other)?;
    {
        let mut f = fs::File::create(&tmp)?;
        if let Err(e) = f.write_all(&body).and_then(|_| f.sync_all()) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    }
    let result = (|| {
        if file.exists() && read_value(file).is_some() {
            fs::copy(file, with_suffix(file, ".bak"))?;
        }
        fs::rename(&tmp, file)
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    // Sync the folder too so the rename itself is durable (not supported on Windows).
    #[cfg(unix)]
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Reads a state file. If the main file is broken, `.bak` is tried. If neither
/// can be read the fallback is returned; the broken file is not deleted but set
/// aside as `.bozuk-<time>` so it can be inspected.
pub fn read_json_safe(file: &Path) -> Option<serde_json::Value> {
    let bak = with_suffix(file, ".bak");
    if !file.exists() {
        return read_value(&bak);
    }
    if let Some(v) = read_value(file) {
        return Some(v);
    }
    let backup = read_value(&bak);
    let _ = fs::rename(file, with_suffix(file, &format!(".bozuk-{}", now_ms())));
    eprintln!(
        "[durum] {} okunamadı; {} devam ediliyor.",
        file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        if backup.is_some() { "yedekten" } else { "varsayılanlarla" }
    );
    backup
}

pub fn read_typed<T: DeserializeOwned>(file: &Path) -> Option<T> {
    read_json_safe(file).and_then(|v| serde_json::from_value(v).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_keeps_backup_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state.json");
        write_json_atomic(&file, &serde_json::json!({"a": 1})).unwrap();
        write_json_atomic(&file, &serde_json::json!({"a": 2})).unwrap();
        assert_eq!(read_json_safe(&file).unwrap()["a"], 2);
        assert_eq!(read_value(&with_suffix(&file, ".bak")).unwrap()["a"], 1);
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn broken_file_is_read_from_backup_and_kept_aside() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("downloads.json");
        write_json_atomic(&file, &serde_json::json!([1])).unwrap();
        write_json_atomic(&file, &serde_json::json!([1, 2])).unwrap();
        fs::write(&file, "{yarım").unwrap();
        assert_eq!(read_json_safe(&file).unwrap(), serde_json::json!([1]));
        let kept = fs::read_dir(dir.path()).unwrap().filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains(".bozuk-"));
        assert!(kept, "broken file is not deleted");
    }
}
