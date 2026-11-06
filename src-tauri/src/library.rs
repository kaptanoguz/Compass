//! Library: a poster catalogue of finished downloads and where you stopped
//! watching. It is separate from the download list, so the entry (and the
//! progress) stays even when the download is removed. The key is the infohash;
//! if the same torrent downloads again its entry is updated.

use crate::files::{playable_in_browser, video_type};
use crate::jsonfile::{now_ms, read_json_safe, write_json_atomic};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const PROGRESS_SAVE: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryFile {
    pub index: usize,
    pub name: String,
    pub path: String,
}

/// What a finished download hands to the library.
#[derive(Clone, Debug, Default)]
pub struct LibraryInfo {
    /// Search-result details (year, poster, genres, imdb, page, licence…).
    pub info: Map<String, Value>,
    pub info_hash: Option<String>,
    pub download_id: String,
    pub title: Option<String>,
    pub source: Option<String>,
    pub added_at: i64,
    pub completed_at: Option<i64>,
    pub files: Vec<LibraryFile>,
}

static ARCHIVE_PAGE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^https://archive\.org/details/([^/?#]+)").unwrap());

/// Archive's own cover for old Archive entries added without a poster.
fn poster_from_page(page: Option<&str>) -> Option<String> {
    ARCHIVE_PAGE.captures(page?).map(|c| format!("https://archive.org/services/img/{}", &c[1]))
}

struct State {
    entries: IndexMap<String, Value>,
    timer: bool,
    last_save: i64,
}

#[derive(Clone)]
pub struct Library {
    file: PathBuf,
    save_delay: Duration,
    st: Arc<Mutex<State>>,
}

pub struct OpenFile {
    pub name: String,
    pub size: u64,
    pub mime: String,
    pub path: PathBuf,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).filter(|x| !x.is_empty()).map(String::from)
}

impl Library {
    pub fn open(file: PathBuf, save_delay: Duration) -> Self {
        let entries = match read_json_safe(&file) {
            Some(Value::Object(m)) => m.into_iter().collect(),
            _ => IndexMap::new(),
        };
        Library { file, save_delay, st: Arc::new(Mutex::new(State { entries, timer: false, last_save: 0 })) }
    }

    fn save_locked(&self, st: &mut State) {
        st.timer = false;
        st.last_save = now_ms();
        if let Err(e) = write_json_atomic(&self.file, &st.entries) {
            eprintln!("[kütüphane] kaydedilemedi: {e}");
        }
    }

    /// The player sends its position every few seconds; the disk is written at most every 5 s.
    fn save_soon(&self, st: &mut State) {
        if st.timer {
            return;
        }
        st.timer = true;
        let wait = (self.save_delay.as_millis() as i64 - (now_ms() - st.last_save)).max(0) as u64;
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(wait)).await;
            let mut st = me.st.lock().unwrap();
            if st.timer {
                me.save_locked(&mut st);
            }
        });
    }

    fn view(entry: &Value) -> Value {
        let mut out = entry.as_object().cloned().unwrap_or_default();
        let items: Vec<Value> = entry["items"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|mut item| {
                let path = item.as_object_mut().and_then(|o| o.remove("path")).and_then(|p| p.as_str().map(String::from));
                let name = s(&item, "name").unwrap_or_default();
                item["playableInBrowser"] = json!(playable_in_browser(&name));
                item["missing"] = json!(path.as_ref().map(|p| !Path::new(p).exists()).unwrap_or(true));
                item
            })
            .collect();
        let missing = !items.is_empty() && items.iter().all(|i| i["missing"] == json!(true));
        if s(entry, "poster").is_none() {
            out.insert("poster".into(), json!(poster_from_page(entry["pageUrl"].as_str())));
        }
        out.insert("items".into(), Value::Array(items));
        out.insert("missing".into(), json!(missing));
        Value::Object(out)
    }

    /// Creates or updates the entry for a finished download. Earlier watch positions are kept.
    pub fn upsert(&self, info: &LibraryInfo) -> Option<Value> {
        let hash = info.info_hash.clone()?;
        let mut st = self.st.lock().unwrap();
        let old = st.entries.get(&hash).cloned().unwrap_or(Value::Null);
        let prev: Vec<Value> = old["items"].as_array().cloned().unwrap_or_default();
        let pick = |k: &str| info.info.get(k).filter(|v| !v.is_null() && v.as_str() != Some("")).cloned().or_else(|| old.get(k).filter(|v| !v.is_null()).cloned()).unwrap_or(Value::Null);
        let genres = match info.info.get("genres") {
            Some(Value::Array(a)) if !a.is_empty() => Value::Array(a.clone()),
            _ => old.get("genres").cloned().filter(|v| v.is_array()).unwrap_or(json!([])),
        };
        let now = now_ms();
        let entry = json!({
            "infoHash": hash,
            "title": info.title.clone().or_else(|| s(&old, "title")).unwrap_or_else(|| "Adsız".into()),
            "year": info.info.get("year").cloned().filter(|v| !v.is_null()).or_else(|| old.get("year").cloned()).unwrap_or(Value::Null),
            "poster": pick("poster"),
            "genres": genres,
            "source": info.source.clone().map(Value::from).or_else(|| old.get("source").cloned()).unwrap_or(Value::Null),
            "imdbId": pick("imdbId"),
            "imdbUrl": pick("imdbUrl"),
            "pageUrl": pick("pageUrl"),
            "license": pick("license"),
            "addedAt": old.get("addedAt").and_then(Value::as_i64).unwrap_or(if info.added_at > 0 { info.added_at } else { now }),
            "completedAt": info.completed_at.or_else(|| old.get("completedAt").and_then(Value::as_i64)).unwrap_or(now),
            "items": info.files.iter().map(|f| {
                let p = prev.iter().find(|i| i["fileIndex"].as_u64() == Some(f.index as u64)).cloned().unwrap_or(Value::Null);
                json!({
                    "downloadId": info.download_id,
                    "fileIndex": f.index,
                    "name": f.name,
                    "path": f.path,
                    "positionSec": p.get("positionSec").cloned().filter(|v| v.is_number()).unwrap_or(json!(0)),
                    "durationSec": p.get("durationSec").cloned().filter(|v| v.is_number()).unwrap_or(json!(0)),
                    "watched": p.get("watched").and_then(Value::as_bool).unwrap_or(false),
                })
            }).collect::<Vec<_>>(),
        });
        let mut entry = entry;
        if let Some(lp) = old.get("lastPlayedAt").filter(|v| !v.is_null()) {
            entry["lastPlayedAt"] = lp.clone();
        }
        st.entries.insert(hash.clone(), entry.clone());
        self.save_locked(&mut st);
        Some(Self::view(&entry))
    }

    pub fn has(&self, hash: &str) -> bool {
        self.st.lock().unwrap().entries.contains_key(hash)
    }

    pub fn list(&self) -> Vec<Value> {
        let st = self.st.lock().unwrap();
        let mut out: Vec<Value> = st.entries.values().map(Self::view).collect();
        let at = |v: &Value| v["lastPlayedAt"].as_f64().filter(|n| *n != 0.0).or_else(|| v["completedAt"].as_f64()).unwrap_or(0.0);
        out.sort_by(|a, b| at(b).partial_cmp(&at(a)).unwrap_or(std::cmp::Ordering::Equal));
        out
    }

    pub fn progress(&self, hash: &str, body: &Value) -> Option<Value> {
        let mut st = self.st.lock().unwrap();
        let entry = st.entries.get_mut(hash)?;
        let index = match &body["fileIndex"] {
            Value::Number(n) => n.as_f64()?,
            Value::String(s) => s.parse().ok()?,
            _ => return None,
        };
        let item = entry["items"].as_array_mut()?.iter_mut().find(|i| i["fileIndex"].as_f64() == Some(index))?;
        if let Some(p) = body["positionSec"].as_f64().filter(|p| *p >= 0.0) {
            item["positionSec"] = json!(p.round() as i64);
        }
        if let Some(d) = body["durationSec"].as_f64().filter(|d| *d > 0.0) {
            item["durationSec"] = json!(d.round() as i64);
        }
        if let Some(w) = body["watched"].as_bool() {
            item["watched"] = json!(w);
        }
        entry["lastPlayedAt"] = json!(now_ms());
        let view = Self::view(entry);
        self.save_soon(&mut st);
        Some(view)
    }

    /// Plays a file that is on disk even when its download was removed from the list.
    pub fn open_file(&self, hash: &str, index: usize) -> Option<OpenFile> {
        let st = self.st.lock().unwrap();
        let item = st.entries.get(hash)?["items"].as_array()?.iter().find(|i| i["fileIndex"].as_u64() == Some(index as u64))?.clone();
        let path = PathBuf::from(s(&item, "path")?);
        let meta = std::fs::metadata(&path).ok()?;
        Some(OpenFile {
            name: s(&item, "name").unwrap_or_default(),
            size: meta.len(),
            mime: video_type(&path.to_string_lossy()).unwrap_or("application/octet-stream").into(),
            path,
        })
    }

    pub fn close(&self) {
        let mut st = self.st.lock().unwrap();
        if st.timer {
            self.save_locked(&mut st);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(dir: &Path) -> LibraryInfo {
        let video = dir.join("film.mp4");
        std::fs::write(&video, b"x").unwrap();
        LibraryInfo {
            info: json!({"year": 1922, "pageUrl": "https://archive.org/details/nosferatu"}).as_object().unwrap().clone(),
            info_hash: Some("h1".into()),
            download_id: "d1".into(),
            title: Some("Nosferatu".into()),
            files: vec![LibraryFile { index: 0, name: "film.mp4".into(), path: video.to_string_lossy().into() }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn progress_is_kept_across_restarts_and_when_the_entry_is_updated() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("library.json");
        let lib = Library::open(file.clone(), Duration::from_millis(20));
        let v = lib.upsert(&info(dir.path())).unwrap();
        assert_eq!(v["poster"], "https://archive.org/services/img/nosferatu", "Archive entries get a poster");
        assert_eq!(v["items"][0]["missing"], false);
        assert!(v["items"][0].get("path").is_none(), "full path is not sent to the page");
        lib.progress("h1", &json!({"fileIndex": 0, "positionSec": 61.4, "durationSec": 3600})).unwrap();
        lib.close();
        let again = Library::open(file, Duration::from_millis(20));
        assert_eq!(again.list()[0]["items"][0]["positionSec"], 61);
        again.upsert(&info(dir.path()));
        assert_eq!(again.list()[0]["items"][0]["positionSec"], 61, "re-download keeps the position");
        assert!(again.open_file("h1", 0).is_some());
        assert!(again.progress("yok", &json!({"fileIndex": 0})).is_none());
    }

    #[tokio::test]
    async fn watch_position_is_written_at_most_once_per_interval() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("library.json");
        let lib = Library::open(file.clone(), Duration::from_millis(200));
        lib.upsert(&info(dir.path()));
        let written = || std::fs::metadata(&file).unwrap().modified().unwrap();
        let first = written();
        tokio::time::sleep(Duration::from_millis(20)).await;
        lib.progress("h1", &json!({"fileIndex": 0, "positionSec": 5}));
        lib.progress("h1", &json!({"fileIndex": 0, "positionSec": 6}));
        assert_eq!(written(), first, "not written at once");
        tokio::time::sleep(Duration::from_millis(400)).await;
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["h1"]["items"][0]["positionSec"], 6);
    }
}
