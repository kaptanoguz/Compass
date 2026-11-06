//! User settings, stored in settings.json. Files from older versions still
//! open: missing fields are filled with defaults and unknown fields are kept.

use crate::jsonfile::{read_json_safe, write_json_atomic};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Torznab {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub download_dir: String,
    pub torznab: Vec<Torznab>,
    pub archive_enabled: bool,
    pub omdb_api_key: String,
    /// Keep sharing finished downloads (ratio on private trackers).
    pub seed_after_done: bool,
    /// How many downloads run at once (the rest wait in the queue).
    pub max_active_downloads: u32,
    /// KB/s; 0 = unlimited.
    pub download_limit_kbps: u64,
    pub upload_limit_kbps: u64,
    /// Keep running in the tray when the window is closed (optional).
    pub close_to_tray: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

pub fn default_download_dir() -> String {
    let base = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("Downloads"));
    base.join("Torrent").to_string_lossy().into_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            download_dir: default_download_dir(),
            torznab: vec![],
            archive_enabled: true,
            omdb_api_key: String::new(),
            seed_after_done: false,
            max_active_downloads: 3,
            download_limit_kbps: 0,
            upload_limit_kbps: 0,
            close_to_tray: false,
            extra: Map::new(),
        }
    }
}

/// Number-like value clamped into [min, max]; None when it is not a number.
fn int_in(v: &Value, min: f64, max: f64) -> Option<f64> {
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if !s.trim().is_empty() => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !n.is_finite() {
        return None;
    }
    Some(n.round().clamp(min, max))
}

fn merge_defaults(stored: Option<Value>) -> Settings {
    let mut base = serde_json::to_value(Settings::default()).unwrap();
    if let (Some(Value::Object(s)), Value::Object(b)) = (stored, &mut base) {
        for (k, v) in s {
            b.insert(k, v);
        }
    }
    serde_json::from_value(base.clone()).unwrap_or_else(|_| {
        // A field with the wrong type: keep what parses, default the rest.
        let mut out = Settings::default();
        if let Value::Object(b) = base {
            let get = |k: &str| b.get(k).cloned();
            if let Some(Value::String(s)) = get("downloadDir") { out.download_dir = s; }
            if let Some(Value::Bool(v)) = get("archiveEnabled") { out.archive_enabled = v; }
            if let Some(Value::Bool(v)) = get("seedAfterDone") { out.seed_after_done = v; }
            if let Some(Value::Bool(v)) = get("closeToTray") { out.close_to_tray = v; }
            if let Some(Value::String(s)) = get("omdbApiKey") { out.omdb_api_key = s; }
            if let Some(t) = get("torznab").and_then(|v| serde_json::from_value(v).ok()) { out.torznab = t; }
        }
        out
    })
}

pub struct SettingsStore {
    file: PathBuf,
    data: Mutex<Settings>,
}

impl SettingsStore {
    pub fn open(file: PathBuf) -> Self {
        let data = merge_defaults(read_json_safe(&file));
        SettingsStore { file, data: Mutex::new(data) }
    }

    pub fn get(&self) -> Settings {
        self.data.lock().unwrap().clone()
    }

    pub fn update(&self, patch: &Value) -> std::io::Result<Settings> {
        let mut guard = self.data.lock().unwrap();
        let mut next = guard.clone();
        let p = |k: &str| patch.get(k);
        if let Some(Value::String(s)) = p("downloadDir") {
            if !s.trim().is_empty() {
                next.download_dir = s.trim().to_string();
            }
        }
        if let Some(Value::Bool(v)) = p("archiveEnabled") { next.archive_enabled = *v; }
        if let Some(Value::Bool(v)) = p("seedAfterDone") { next.seed_after_done = *v; }
        if let Some(Value::Bool(v)) = p("closeToTray") { next.close_to_tray = *v; }
        if let Some(Value::String(s)) = p("omdbApiKey") { next.omdb_api_key = s.trim().to_string(); }
        if let Some(n) = p("maxActiveDownloads").and_then(|v| int_in(v, 1.0, 10.0)) {
            next.max_active_downloads = n as u32;
        }
        if let Some(n) = p("downloadLimitKbps").and_then(|v| int_in(v, 0.0, 1e6)) { next.download_limit_kbps = n as u64; }
        if let Some(n) = p("uploadLimitKbps").and_then(|v| int_in(v, 0.0, 1e6)) { next.upload_limit_kbps = n as u64; }
        if let Some(Value::Array(list)) = p("torznab") {
            let url_ok = regex::Regex::new(r"(?i)^https?://").unwrap();
            next.torznab = list
                .iter()
                .filter(|t| t.get("url").and_then(Value::as_str).is_some_and(|u| url_ok.is_match(u.trim())))
                .enumerate()
                .map(|(i, t)| {
                    let s = |k: &str| match t.get(k) {
                        Some(Value::String(s)) => s.trim().to_string(),
                        Some(Value::Number(n)) => n.to_string(),
                        _ => String::new(),
                    };
                    let name = s("name");
                    Torznab {
                        id: format!("torznab-{i}"),
                        name: if name.is_empty() { format!("Kaynak {}", i + 1) } else { name },
                        url: s("url"),
                        api_key: s("apiKey"),
                        enabled: t.get("enabled") != Some(&Value::Bool(false)),
                    }
                })
                .collect();
        }
        write_json_atomic(&self.file, &next)?;
        *guard = next.clone();
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn old_settings_file_gets_defaults_and_keeps_its_values() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        std::fs::write(&file, r#"{"downloadDir":"/tmp/x","archiveEnabled":false,"eski":1}"#).unwrap();
        let s = SettingsStore::open(file).get();
        assert_eq!(s.download_dir, "/tmp/x");
        assert!(!s.archive_enabled);
        assert_eq!(s.max_active_downloads, 3);
        assert!(!s.close_to_tray, "close to tray is off by default");
        assert_eq!(s.extra.get("eski"), Some(&json!(1)));
    }

    #[test]
    fn queue_and_speed_settings_are_clamped() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(dir.path().join("settings.json"));
        let s = store.update(&json!({"maxActiveDownloads": 99, "downloadLimitKbps": -5, "uploadLimitKbps": "250"})).unwrap();
        assert_eq!(s.max_active_downloads, 10);
        assert_eq!(s.download_limit_kbps, 0);
        assert_eq!(s.upload_limit_kbps, 250);
        let s = store.update(&json!({"maxActiveDownloads": "abc"})).unwrap();
        assert_eq!(s.max_active_downloads, 10, "invalid value is ignored");
    }

    #[test]
    fn torznab_entries_are_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let store = SettingsStore::open(dir.path().join("settings.json"));
        let s = store
            .update(&json!({"torznab": [{"url": "ftp://x"}, {"url": " http://localhost:9117/api ", "apiKey": " k "}]}))
            .unwrap();
        assert_eq!(s.torznab.len(), 1);
        assert_eq!(s.torznab[0].id, "torznab-0");
        assert_eq!(s.torznab[0].name, "Kaynak 1");
        assert_eq!(s.torznab[0].url, "http://localhost:9117/api");
        assert_eq!(s.torznab[0].api_key, "k");
        assert!(s.torznab[0].enabled);
    }
}
