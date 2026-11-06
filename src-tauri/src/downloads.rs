//! Download manager around the engine (qBittorrent). The list is saved to
//! disk, so downloads go on where they left off after the app restarts.

use crate::files::{file_kind, playable_in_browser, screen_programs, subtitle_to_vtt, video_type, Kind, Screen, BLOCKED_MESSAGE, MAX_SUBTITLE_BYTES};
use crate::jsonfile::{now_ms, read_json_safe, write_json_atomic};
use crate::library::{LibraryFile, LibraryInfo};
use crate::qbittorrent::{Engine, Event, Source, Torrent};
use crate::torrent::{magnet_info_hash, parse_torrent};
use base64::Engine as _;
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

// Dropped connections, DNS or timeouts: errors that may fix themselves.
static TRANSIENT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)fetch failed|ENOTFOUND|EAI_AGAIN|ECONNREFUSED|ECONNRESET|ECONNABORTED|ETIMEDOUT|ENETUNREACH|ENETDOWN|EHOSTUNREACH|EPIPE|socket hang up|network|timed?[ -]?out|Error downloading torrent").unwrap()
});

pub fn is_transient_error(message: &str) -> bool {
    TRANSIENT.is_match(message)
}

pub const MAX_RETRIES: u32 = 5;
const SEED_CHECK: Duration = Duration::from_secs(2);
const SUBTITLE_WAIT: Duration = Duration::from_secs(20);

fn friendly_error(message: &str) -> String {
    let m = message;
    let has = |re: &str| Regex::new(re).unwrap().is_match(m);
    if has("(?i)duplicate") {
        return "Bu torrent zaten listede.".into();
    }
    if has("(?i)Error downloading torrent|fetch failed") {
        return "Torrent dosyası alınamadı. İnternet bağlantını kontrol edip tekrar dene.".into();
    }
    if is_transient_error(m) {
        return "Bağlantı koptu ya da zaman aşımına uğradı. İnternet bağlantını kontrol edip tekrar dene.".into();
    }
    if has("(?i)Invalid torrent identifier|Torrent is missing required field") {
        return "Geçersiz torrent dosyası.".into();
    }
    if has("ENOSPC|No space left") {
        return "Diskte yer kalmadı.".into();
    }
    if has("EACCES|EPERM|Permission denied") {
        return "İndirme klasörüne yazılamıyor. Ayarlar’dan başka bir klasör seç.".into();
    }
    m.to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileMeta {
    pub name: String,
    pub path: String,
    pub length: u64,
}

/// One saved download (downloads.json). Unknown fields from older versions are kept.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default)]
    pub info: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub magnet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torrent_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_indexes: Option<Vec<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped_programs: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick_files: Option<bool>,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub added_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileMeta>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_count: Option<u32>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Meta {
    fn picking(&self) -> bool {
        self.pick_files == Some(true)
    }
    fn files(&self) -> &[FileMeta] {
        self.files.as_deref().unwrap_or(&[])
    }
    fn is_selected(&self, i: usize) -> bool {
        self.selected_indexes.as_ref().is_none_or(|s| s.contains(&i))
    }
    fn file_path(&self, f: &FileMeta) -> PathBuf {
        Path::new(&self.path).join(&f.path)
    }
    /// Only a torrent picked from the Software category may download programs.
    fn allows_programs(&self) -> bool {
        self.info.get("category").and_then(Value::as_str) == Some("software")
    }
}

/// Details from the search result, shown in the library. They come from the
/// page, so types and lengths are limited.
fn clean_info(r: &Map<String, Value>) -> Map<String, Value> {
    let st = |k: &str, max: usize| r.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(max).collect::<String>());
    let url = |k: &str| st(k, 1000).filter(|s| s.starts_with("https://"));
    let mut out = Map::new();
    if let Some(y) = r.get("year").and_then(Value::as_i64).filter(|y| *y > 1800 && *y < 2200) {
        out.insert("year".into(), json!(y));
    }
    for k in ["poster", "imdbUrl", "pageUrl"] {
        if let Some(u) = url(k) {
            out.insert(k.into(), json!(u));
        }
    }
    if let Some(Value::Array(g)) = r.get("genres") {
        let g: Vec<String> = g.iter().filter_map(Value::as_str).take(10).map(|s| s.chars().take(40).collect()).collect();
        out.insert("genres".into(), json!(g));
    }
    if let Some(id) = r.get("imdbId").and_then(Value::as_str).filter(|s| Regex::new(r"^tt\d{5,10}$").unwrap().is_match(s)) {
        out.insert("imdbId".into(), json!(id));
    }
    if let Some(l) = st("license", 40) {
        out.insert("license".into(), json!(l));
    }
    if let Some(c) = r.get("category").and_then(Value::as_str) {
        if ["movies", "tv", "music", "books", "software", "adult", "audio", "texts"].contains(&c) {
            out.insert("category".into(), json!(c));
        }
    }
    out
}

/// File indexes from the page: not empty, whole numbers, no repeats, sorted.
fn clean_indexes(list: &Value) -> Result<Vec<usize>, String> {
    let arr = list.as_array().filter(|a| !a.is_empty()).ok_or("En az bir dosya seçin.")?;
    let mut out: Vec<usize> = arr.iter().map(|v| v.as_u64().map(|n| n as usize).ok_or("Geçersiz dosya seçimi.".to_string())).collect::<Result<_, _>>()?;
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

pub struct Hooks {
    pub download_dir: Box<dyn Fn() -> String + Send + Sync>,
    pub should_seed: Box<dyn Fn() -> bool + Send + Sync>,
    pub max_active: Box<dyn Fn() -> usize + Send + Sync>,
    pub on_done: Box<dyn Fn(LibraryInfo) + Send + Sync>,
}

struct Item {
    meta: Meta,
    torrent: Option<Arc<Torrent>>,
    seed: bool,
    partial: bool,
    finished: bool,
    retry: Option<tokio::task::AbortHandle>,
}

impl Item {
    fn new(meta: Meta) -> Self {
        Item { meta, torrent: None, seed: false, partial: false, finished: false, retry: None }
    }
    fn live(&self) -> Option<&Arc<Torrent>> {
        self.torrent.as_ref().filter(|t| !t.destroyed())
    }
}

struct Inner {
    state_file: PathBuf,
    engine: Engine,
    hooks: Hooks,
    retry_base: Duration,
    items: Mutex<IndexMap<String, Item>>,
}

type Items = IndexMap<String, Item>;

#[derive(Clone)]
pub struct Downloads {
    inner: Arc<Inner>,
}

pub enum OpenedFile {
    Disk { name: String, size: u64, mime: String, path: PathBuf },
    Live { name: String, size: u64, mime: String, torrent: Arc<Torrent>, index: usize },
}

pub struct Inspected {
    pub json: Value,
    pub info_hash: String,
    pub names: Vec<String>,
    pub file_count: usize,
    pub name: String,
    pub length: u64,
}

impl Inner {
    fn save(&self, items: &Items) {
        let list: Vec<&Meta> = items.values().map(|i| &i.meta).collect();
        if let Err(e) = write_json_atomic(&self.state_file, &list) {
            eprintln!("[indirme] liste kaydedilemedi: {e}");
        }
    }

    /// seed: opens a finished torrent only to share it. No piece is selected, so
    /// missing or moved files are never downloaded again.
    /// Without selectedIndexes every file downloads (old entries stay like that).
    /// pickFiles: the file list of a magnet/URL is not known yet and the user will choose.
    fn attach(&self, item: &mut Item, seed: bool) {
        let meta = &mut item.meta;
        let source = if let Some(b64) = &meta.torrent_file {
            match base64::engine::general_purpose::STANDARD.decode(b64) {
                Ok(b) => Source::Bytes(b),
                Err(_) => Source::Magnet(meta.magnet.clone().or(meta.torrent_url.clone()).unwrap_or_default()),
            }
        } else {
            Source::Magnet(meta.magnet.clone().or(meta.torrent_url.clone()).unwrap_or_default())
        };
        if !seed {
            meta.status = "connecting".into();
            meta.error = None;
        }
        item.partial = meta.selected_indexes.is_some();
        let deselect = seed || item.partial || meta.picking();
        item.torrent = Some(self.engine.add(source, &meta.path, deselect));
        item.seed = seed;
        item.finished = false;
    }

    /// forget: the torrent leaves the engine entirely (files stay). Finished, removed and
    /// blocked downloads close like this, so moved files are never downloaded again.
    /// Paused ones wait in the engine and resume without re-checking files.
    fn detach(&self, item: &mut Item, forget: bool) {
        if let Some(t) = item.torrent.take() {
            item.meta.uploaded = Some(item.meta.uploaded.unwrap_or(0) + t.state().uploaded);
            if forget {
                self.engine.remove(&t)
            } else {
                self.engine.pause(&t)
            }
        }
    }

    /// Queue: at most max_active() downloads connect/download at once. Downloads waiting
    /// for a file choice, paused and seeding (finished) ones don't take a place.
    fn active_count(items: &Items) -> usize {
        items.values().filter(|i| i.live().is_some() && i.meta.status != "done" && i.meta.status != "picking").count()
    }

    fn start_or_queue(&self, items: &mut Items, id: &str, force: bool) {
        if !force && Self::active_count(items) >= (self.hooks.max_active)() {
            let item = items.get_mut(id).unwrap();
            item.meta.status = "queued".into();
            item.meta.error = None;
            return;
        }
        self.attach(items.get_mut(id).unwrap(), false);
    }

    /// When a place frees up, start the queued ones (in the order they were added).
    fn pump(&self, items: &mut Items) {
        let mut queued: Vec<(i64, String)> =
            items.values().filter(|i| i.meta.status == "queued" && i.torrent.is_none()).map(|i| (i.meta.added_at, i.meta.id.clone())).collect();
        queued.sort();
        let mut started = false;
        for (_, id) in queued {
            if Self::active_count(items) >= (self.hooks.max_active)() {
                break;
            }
            self.attach(items.get_mut(&id).unwrap(), false);
            started = true;
        }
        if started {
            self.save(items);
        }
    }

    fn selected_files(meta: &Meta) -> Vec<(usize, FileMeta)> {
        meta.files().iter().cloned().enumerate().filter(|(i, _)| meta.is_selected(*i)).collect()
    }

    /// All selected files on disk at the expected size: can be shared.
    fn complete(meta: &Meta) -> bool {
        let files = Self::selected_files(meta);
        !files.is_empty() && files.iter().all(|(_, f)| std::fs::metadata(meta.file_path(f)).map(|m| m.len() == f.length).unwrap_or(false))
    }

    /// For the library entry: the selected videos with full paths.
    fn library_info(meta: &Meta) -> LibraryInfo {
        LibraryInfo {
            info: meta.info.clone(),
            info_hash: meta.info_hash.clone(),
            download_id: meta.id.clone(),
            title: meta.title.clone().or(meta.name.clone()),
            source: meta.source.clone(),
            added_at: meta.added_at,
            completed_at: meta.completed_at,
            files: Self::selected_files(meta)
                .into_iter()
                .filter(|(_, f)| file_kind(&f.name) == Kind::Video)
                .map(|(index, f)| LibraryFile { index, name: f.name.clone(), path: meta.file_path(&f).to_string_lossy().into() })
                .collect(),
        }
    }

    fn describe(t: &Torrent) -> Vec<FileMeta> {
        t.state().files.iter().map(|f| FileMeta { name: f.name.clone(), path: f.path.clone(), length: f.length }).collect()
    }

    fn file_list(meta: &Meta) -> Vec<Value> {
        let done = meta.status == "done";
        meta.files()
            .iter()
            .enumerate()
            .map(|(index, f)| {
                json!({
                    "index": index,
                    "name": f.name,
                    "path": f.path,
                    "length": f.length,
                    "kind": file_kind(&f.name),
                    "selected": meta.is_selected(index),
                    "exists": if done { json!(meta.file_path(f).exists()) } else { Value::Null },
                })
            })
            .collect()
    }

    /// With a selection, progress counts the selected files only.
    fn progress_of(item: &Item) -> (f64, u64, u64) {
        let s = item.torrent.as_ref().map(|t| t.state()).unwrap_or_default();
        match &item.meta.selected_indexes {
            Some(sel) if !s.files.is_empty() => {
                let chosen: Vec<_> = sel.iter().filter_map(|&i| s.files.get(i)).collect();
                let length: u64 = chosen.iter().map(|f| f.length).sum();
                let downloaded: u64 = chosen.iter().map(|f| f.downloaded.min(f.length)).sum();
                (if length > 0 { downloaded as f64 / length as f64 } else { 0.0 }, downloaded, length)
            }
            _ => (s.progress, s.downloaded, s.length),
        }
    }

    fn snapshot(item: &Item) -> Value {
        let meta = &item.meta;
        let files: Vec<Value> = Self::file_list(meta).into_iter().filter(|f| f["selected"] == json!(true)).collect();
        let missing = meta.status == "done" && !files.is_empty() && files.iter().all(|f| f["exists"] != json!(true));
        let live = item.live();
        let st = live.map(|t| t.state());
        let uploaded = meta.uploaded.unwrap_or(0) + st.as_ref().map(|s| s.uploaded).unwrap_or(0);
        let has_meta = st.as_ref().is_some_and(|s| s.length > 0);
        let prog = live.map(|_| Self::progress_of(item));
        let selected_length: Option<u64> =
            (meta.selected_indexes.is_some() && meta.files.is_some()).then(|| files.iter().map(|f| f["length"].as_u64().unwrap_or(0)).sum());
        let mut status = meta.status.clone();
        if let Some(s) = &st {
            if status != "done" && status != "error" && status != "picking" {
                status = if s.done { "done" } else if has_meta || status == "downloading" { "downloading" } else { "connecting" }.into();
            }
        }
        let total = selected_length.or(meta.length);
        let length = prog.map(|p| p.2).filter(|n| *n > 0).or(selected_length.filter(|n| *n > 0)).or(meta.length);
        json!({
            "id": meta.id,
            "key": meta.magnet.clone().or(meta.torrent_url.clone()).unwrap_or_else(|| format!("infohash:{}", meta.info_hash.clone().unwrap_or_default())),
            "infoHash": meta.info_hash,
            "fileCount": meta.files().len(),
            "skippedPrograms": meta.skipped_programs.unwrap_or(0),
            "retry": if status == "connecting" { meta.retry_count.unwrap_or(0) } else { 0 },
            "title": meta.title.clone().or(meta.name.clone()).unwrap_or_else(|| "İndirme".into()),
            "name": meta.name,
            "source": meta.source,
            "status": status,
            "error": meta.error,
            "progress": match prog { Some(p) => p.0, None if meta.status == "done" => 1.0, None => meta.progress.unwrap_or(0.0) },
            "downloaded": prog.map(|p| p.1),
            "length": length,
            "downloadSpeed": st.as_ref().map(|s| s.download_speed).unwrap_or(0),
            "uploadSpeed": st.as_ref().map(|s| s.upload_speed).unwrap_or(0),
            "timeRemaining": st.as_ref().and_then(|s| s.time_remaining),
            "peers": st.as_ref().map(|s| s.peers).unwrap_or(0),
            "seeding": live.is_some() && meta.status == "done" && files.iter().all(|f| f["exists"] == json!(true)),
            "uploaded": uploaded,
            "ratio": match total { Some(t) if t > 0 => uploaded as f64 / t as f64, _ => 0.0 },
            "path": meta.path,
            "addedAt": meta.added_at,
            "completedAt": meta.completed_at,
            "missing": missing,
            // Watchable videos: they open as a stream while downloading too.
            "videos": files.iter().filter(|f| f["kind"] == "video" && f["exists"] != json!(false)).map(|f| json!({
                "index": f["index"], "name": f["name"], "length": f["length"],
                "playableInBrowser": playable_in_browser(f["name"].as_str().unwrap_or_default()),
            })).collect::<Vec<_>>(),
        })
    }

    // ------------------------------------------------------------ events

    fn on_event(self: &Arc<Self>, tid: u64, ev: Event) {
        let mut items = self.items.lock().unwrap();
        let Some(id) = items.values().find(|i| i.torrent.as_ref().is_some_and(|t| t.id == tid)).map(|i| i.meta.id.clone()) else { return };
        match ev {
            Event::Metadata => self.on_metadata(&mut items, &id),
            Event::FileDone(i) => {
                let item = &items[&id];
                if !item.seed && item.meta.selected_indexes.as_ref().is_some_and(|s| s.contains(&i)) {
                    self.check_selected(&mut items, &id);
                }
            }
            Event::Done => self.finish(&mut items, &id),
            Event::Error(msg) => self.on_error(&mut items, &id, &msg),
        }
    }

    fn on_metadata(self: &Arc<Self>, items: &mut Items, id: &str) {
        let item = items.get_mut(id).unwrap();
        let t = item.torrent.clone().unwrap();
        let st = t.state();
        let seed = item.seed;
        let meta = &mut item.meta;
        meta.name = st.name.clone();
        meta.info_hash = st.info_hash.clone();
        meta.length = Some(st.length);
        meta.files = Some(Self::describe(&t));
        if meta.torrent_file.is_none() {
            if let Some(b) = &st.torrent_file {
                meta.torrent_file = Some(base64::engine::general_purpose::STANDARD.encode(b));
            }
        }
        let names: Vec<String> = st.files.iter().map(|f| f.name.clone()).collect();
        let screen = if !seed && !meta.allows_programs() { screen_programs(&names) } else { None };
        match screen {
            Some(Screen::Blocked) => {
                // A fake "film": stopped before any piece downloads.
                meta.status = "error".into();
                meta.error = Some(BLOCKED_MESSAGE.into());
                self.detach(item, true);
                self.save(items);
                self.pump(items);
                return;
            }
            Some(Screen::Keep { keep, skipped }) => {
                meta.skipped_programs = Some(skipped);
                // When the user will be asked, program files show as disabled in the
                // window; otherwise they are removed from the selection.
                if !(meta.picking() && keep.len() > 1) {
                    if !item.partial && !meta.picking() {
                        self.engine.deselect_all(&t);
                    }
                    let chosen: Vec<usize> = meta.selected_indexes.clone().unwrap_or_else(|| keep.clone()).into_iter().filter(|i| keep.contains(i)).collect();
                    meta.selected_indexes = Some(if chosen.is_empty() { keep } else { chosen });
                    meta.pick_files = None;
                }
            }
            None => {}
        }
        if !seed {
            if let Some(sel) = &mut meta.selected_indexes {
                sel.retain(|i| *i < st.files.len());
                if sel.is_empty() {
                    meta.selected_indexes = None;
                }
            }
        }
        if !seed && meta.selected_indexes.is_some() {
            self.engine.select_files(&t, meta.selected_indexes.clone().unwrap());
        } else if !seed && meta.picking() {
            if st.files.len() > 1 {
                // Several files: nothing downloads until the user chooses.
                meta.status = "picking".into();
                meta.progress = Some(0.0);
                self.detach(item, false);
                self.save(items);
                self.pump(items);
                return;
            }
            self.engine.select_all(&t);
        }
        self.engine.begin(&t);
        self.save(items);
        if !seed && items[id].meta.selected_indexes.is_some() {
            self.check_selected(items, id);
        }
    }

    /// The download is finished when the selected files are.
    fn check_selected(self: &Arc<Self>, items: &mut Items, id: &str) {
        let item = &items[id];
        let Some(t) = &item.torrent else { return };
        let st = t.state();
        let chosen: Vec<_> = Self::selected_files(&item.meta).iter().filter_map(|(i, _)| st.files.get(*i).cloned()).collect();
        if !chosen.is_empty() && chosen.iter().all(|f| f.done()) {
            self.finish(items, id);
        }
    }

    fn finish(self: &Arc<Self>, items: &mut Items, id: &str) {
        let item = items.get_mut(id).unwrap();
        if item.seed || item.finished {
            return;
        }
        item.finished = true;
        let t = item.torrent.clone().unwrap();
        let meta = &mut item.meta;
        meta.retry_count = None;
        meta.status = "done".into();
        meta.progress = Some(1.0);
        meta.completed_at.get_or_insert(now_ms());
        meta.files = Some(Self::describe(&t));
        let info = Self::library_info(meta);
        self.save(items);
        (self.hooks.on_done)(info);
        // With sharing off a finished torrent is closed: even if its files are
        // moved or deleted later, the app never tries to download them again.
        if !(self.hooks.should_seed)() {
            self.detach(items.get_mut(id).unwrap(), true);
        }
        self.pump(items);
    }

    fn on_error(self: &Arc<Self>, items: &mut Items, id: &str, msg: &str) {
        let item = items.get_mut(id).unwrap();
        if item.meta.status == "done" {
            // Sharing error: stop quietly, the download state stays intact.
            self.detach(item, true);
            self.save(items);
            return;
        }
        eprintln!("[indirme] {}: {msg}", label(&item.meta));
        // The engine closes a failed torrent; retrying is adding it again.
        self.detach(item, false);
        if is_transient_error(msg) && item.meta.retry_count.unwrap_or(0) < MAX_RETRIES {
            self.schedule_retry(items, id);
        } else {
            item.meta.status = "error".into();
            item.meta.error = Some(friendly_error(msg));
        }
        self.save(items);
        self.pump(items);
    }

    /// After a temporary error, retries at growing intervals (3 s, 6 s, 12 s…) at most
    /// MAX_RETRIES times. While waiting it shows "Connecting" but takes no place in the queue.
    fn schedule_retry(self: &Arc<Self>, items: &mut Items, id: &str) {
        let item = items.get_mut(id).unwrap();
        let n = item.meta.retry_count.unwrap_or(0) + 1;
        item.meta.retry_count = Some(n);
        item.meta.status = "connecting".into();
        item.meta.error = None;
        let delay = self.retry_base * 2u32.pow(n - 1);
        eprintln!("[indirme] {}: {} sn sonra yeniden denenecek ({n}/{MAX_RETRIES})", label(&item.meta), delay.as_secs_f64());
        if let Some(h) = item.retry.take() {
            h.abort();
        }
        let me = self.clone();
        let id2 = id.to_string();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let mut items = me.items.lock().unwrap();
            let Some(item) = items.get_mut(&id2) else { return };
            item.retry = None;
            // Paused or removed meanwhile: leave it.
            if item.torrent.is_some() || item.meta.status != "connecting" {
                return;
            }
            me.start_or_queue(&mut items, &id2, false);
            me.save(&items);
        });
        items.get_mut(id).unwrap().retry = Some(handle.abort_handle());
    }

    fn cancel_retry(item: &mut Item) {
        if let Some(h) = item.retry.take() {
            h.abort();
        }
    }

    /// Stop sharing if one of the shared files is moved or deleted. Kept on its own
    /// timer so reading the list has no side effects.
    fn stop_broken_seeds(&self) {
        let mut items = self.items.lock().unwrap();
        let broken: Vec<String> =
            items.values().filter(|i| i.torrent.is_some() && i.meta.status == "done" && !Self::complete(&i.meta)).map(|i| i.meta.id.clone()).collect();
        for id in &broken {
            self.detach(items.get_mut(id).unwrap(), true);
        }
        if !broken.is_empty() {
            self.save(&items);
        }
    }
}

fn label(meta: &Meta) -> String {
    meta.title.clone().or(meta.name.clone()).unwrap_or_else(|| meta.id.clone())
}

impl Downloads {
    pub fn new(state_file: PathBuf, engine: Engine, mut events: mpsc::UnboundedReceiver<(u64, Event)>, hooks: Hooks, retry_base: Duration) -> Self {
        let inner = Arc::new(Inner { state_file, engine, hooks, retry_base, items: Mutex::new(IndexMap::new()) });
        // Bring back what was left from the previous session.
        let stored: Vec<Meta> = match read_json_safe(&inner.state_file) {
            Some(Value::Array(a)) => a.into_iter().filter_map(|v| serde_json::from_value::<Meta>(v).ok()).filter(|m| !m.id.is_empty()).collect(),
            _ => vec![],
        };
        let mut stored = stored;
        stored.sort_by_key(|m| m.added_at);
        {
            let mut items = inner.items.lock().unwrap();
            for meta in stored {
                let id = meta.id.clone();
                items.insert(id.clone(), Item::new(meta));
                let m = &items[&id].meta;
                // Finished ones are not reopened, so they are never downloaded again even if moved.
                // One waiting for a file choice stays closed until the user chooses.
                // Failed ones are tried afresh in the new session; the .exe block is permanent.
                if m.status == "error" && m.error.as_deref() != Some(BLOCKED_MESSAGE) {
                    items.get_mut(&id).unwrap().meta.retry_count = Some(0);
                    inner.start_or_queue(&mut items, &id, false);
                } else if !["paused", "done", "error", "picking"].contains(&m.status.as_str()) {
                    inner.start_or_queue(&mut items, &id, false);
                } else if m.status == "done" && (inner.hooks.should_seed)() && Inner::complete(m) {
                    inner.attach(items.get_mut(&id).unwrap(), true);
                }
            }
        }
        let me = inner.clone();
        tokio::spawn(async move {
            while let Some((tid, ev)) = events.recv().await {
                me.on_event(tid, ev);
            }
        });
        let weak = Arc::downgrade(&inner);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(SEED_CHECK).await;
                let Some(me) = weak.upgrade() else { return };
                me.stop_broken_seeds();
            }
        });
        Downloads { inner }
    }

    pub fn list(&self) -> Vec<Value> {
        let items = self.inner.items.lock().unwrap();
        let mut out: Vec<Value> = items.values().map(Inner::snapshot).collect();
        out.sort_by_key(|v| std::cmp::Reverse(v["addedAt"].as_i64().unwrap_or(0)));
        out
    }

    /// Reads a .torrent from the computer (base64) to show it before it is added.
    pub fn inspect(&self, b64: &str) -> Result<Inspected, String> {
        let buf = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| "Geçerli bir .torrent dosyası değil.")?;
        let t = parse_torrent(&buf).map_err(|_| "Geçerli bir .torrent dosyası değil.")?;
        let items = self.inner.items.lock().unwrap();
        let existing = items.values().find(|i| i.meta.info_hash.as_deref() == Some(&t.info_hash)).map(|i| Inner::snapshot(i)["status"].clone());
        let names: Vec<String> = t.files.iter().map(|f| f.name.clone()).collect();
        let json = json!({
            "infoHash": t.info_hash,
            "name": t.name,
            "length": t.length,
            "fileCount": t.files.len(),
            "files": t.files.iter().take(500).enumerate().map(|(index, f)| json!({"index": index, "name": f.name, "path": f.path, "length": f.length, "kind": file_kind(&f.name)})).collect::<Vec<_>>(),
            "private": t.private,
            // A fake torrent with only programs instead of a film.
            "blocked": matches!(screen_programs(&names), Some(Screen::Blocked)),
            "programs": names.iter().filter(|n| file_kind(n) == Kind::Program).count(),
            "existing": existing,
        });
        Ok(Inspected { json, info_hash: t.info_hash, file_count: t.files.len(), names, name: t.name, length: t.length })
    }

    /// Finished downloads (to move old entries without a library into it).
    pub fn completed(&self) -> Vec<LibraryInfo> {
        let items = self.inner.items.lock().unwrap();
        items.values().filter(|i| i.meta.status == "done" && i.meta.info_hash.is_some()).map(|i| Inner::library_info(&i.meta)).collect()
    }

    pub fn add(&self, body: &Value) -> Result<Value, String> {
        let obj = body.as_object().cloned().unwrap_or_default();
        let s = |k: &str| obj.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
        let info = clean_info(&obj);
        let indexes = match obj.get("fileIndexes") {
            None | Some(Value::Null) => None,
            Some(v) => Some(clean_indexes(v)?),
        };
        let dir = (self.inner.hooks.download_dir)();
        if let Some(tf) = s("torrentFile") {
            let t = self.inspect(&tf)?;
            let mut items = self.inner.items.lock().unwrap();
            if let Some(found) = items.values().find(|i| i.meta.info_hash.as_deref() == Some(&t.info_hash)) {
                return Ok(Inner::snapshot(found));
            }
            if indexes.as_ref().is_some_and(|ix| ix.iter().any(|i| *i >= t.file_count)) {
                return Err("Geçersiz dosya seçimi.".into());
            }
            // Program files (.exe etc.) are removed; if no film is left it is not added at all.
            let mut chosen = indexes.clone();
            let mut skipped = None;
            if info.get("category").and_then(Value::as_str) != Some("software") {
                match screen_programs(&t.names) {
                    Some(Screen::Blocked) => return Err(BLOCKED_MESSAGE.into()),
                    Some(Screen::Keep { keep, skipped: n }) => {
                        let c: Vec<usize> = indexes.clone().unwrap_or_else(|| keep.clone()).into_iter().filter(|i| keep.contains(i)).collect();
                        if c.is_empty() {
                            return Err("Seçilen dosyalar çalıştırılabilir dosya; güvenlik için indirilmez.".into());
                        }
                        chosen = Some(c);
                        skipped = Some(n);
                    }
                    None => {}
                }
            }
            let buf = base64::engine::general_purpose::STANDARD.decode(tf.trim()).unwrap_or_default();
            let meta = Meta {
                id: uuid::Uuid::new_v4().to_string(),
                title: s("title").or(Some(t.name.clone())),
                source: s("source").or(Some("Torrent dosyası".into())),
                info,
                torrent_file: Some(base64::engine::general_purpose::STANDARD.encode(buf)),
                info_hash: Some(t.info_hash.clone()),
                name: Some(t.name.clone()),
                length: Some(t.length),
                selected_indexes: chosen.filter(|c| c.len() < t.file_count),
                skipped_programs: skipped,
                path: dir.clone(),
                status: "connecting".into(),
                added_at: now_ms(),
                ..Default::default()
            };
            std::fs::create_dir_all(&dir).map_err(|e| friendly_error(&e.to_string()))?;
            let id = meta.id.clone();
            items.insert(id.clone(), Item::new(meta));
            self.inner.start_or_queue(&mut items, &id, false);
            self.inner.save(&items);
            return Ok(Inner::snapshot(&items[&id]));
        }
        let magnet = s("magnet");
        let url = s("torrentUrl");
        if magnet.is_none() && url.is_none() {
            return Err("Magnet bağlantısı ya da torrent adresi gerekli.".into());
        }
        if magnet.as_ref().is_some_and(|m| !m.to_lowercase().starts_with("magnet:?")) {
            return Err("Geçersiz magnet bağlantısı.".into());
        }
        if url.as_ref().is_some_and(|u| !Regex::new(r"(?i)^https?://").unwrap().is_match(u)) {
            return Err("Geçersiz torrent adresi.".into());
        }
        let key = magnet.clone().or(url.clone()).unwrap();
        let hash = magnet.as_deref().and_then(magnet_info_hash);
        let mut items = self.inner.items.lock().unwrap();
        let same = items
            .values()
            .find(|i| match &hash {
                Some(h) => i.meta.info_hash.as_deref() == Some(h),
                None => i.meta.magnet.clone().or(i.meta.torrent_url.clone()).as_deref() == Some(&key),
            })
            .map(|i| i.meta.id.clone());
        if let Some(id) = same {
            // A download whose files were deleted/moved downloads again when asked on purpose.
            if Inner::snapshot(&items[&id])["missing"] == json!(true) {
                let item = items.get_mut(&id).unwrap();
                item.meta.path = dir.clone();
                item.meta.completed_at = None;
                let _ = std::fs::create_dir_all(&dir);
                self.inner.start_or_queue(&mut items, &id, false);
                self.inner.save(&items);
            }
            return Ok(Inner::snapshot(&items[&id]));
        }
        let meta = Meta {
            id: uuid::Uuid::new_v4().to_string(),
            title: s("title"),
            source: s("source"),
            info,
            magnet,
            torrent_url: url,
            info_hash: hash,
            // Without a selection the user is asked once the file list arrives.
            pick_files: indexes.is_none().then_some(true),
            selected_indexes: indexes,
            path: dir.clone(),
            status: "connecting".into(),
            added_at: now_ms(),
            ..Default::default()
        };
        std::fs::create_dir_all(&dir).map_err(|e| friendly_error(&e.to_string()))?;
        let id = meta.id.clone();
        items.insert(id.clone(), Item::new(meta));
        self.inner.start_or_queue(&mut items, &id, false);
        self.inner.save(&items);
        Ok(Inner::snapshot(&items[&id]))
    }

    /// Turns sharing of finished downloads on or off when the setting changes.
    pub fn set_seeding(&self, on: bool) {
        let mut items = self.inner.items.lock().unwrap();
        let ids: Vec<String> = items.keys().cloned().collect();
        for id in ids {
            let item = items.get_mut(&id).unwrap();
            if item.meta.status != "done" {
                continue;
            }
            if on && item.torrent.is_none() && Inner::complete(&item.meta) {
                self.inner.attach(item, true);
            }
            if !on && item.torrent.is_some() {
                self.inner.detach(item, true);
            }
        }
        self.inner.save(&items);
    }

    /// The list for the file-choice window. Empty for a magnet until its metadata arrives.
    pub fn files(&self, id: &str) -> Option<Value> {
        let items = self.inner.items.lock().unwrap();
        let item = items.get(id)?;
        let snap = Inner::snapshot(item);
        let files: Vec<Value> = Inner::file_list(&item.meta)
            .into_iter()
            .take(500)
            .map(|mut f| {
                f.as_object_mut().unwrap().remove("exists");
                f
            })
            .collect();
        Some(json!({"id": snap["id"], "title": snap["title"], "status": snap["status"], "files": files}))
    }

    /// Download only the chosen files. The choice is saved in downloads.json and the
    /// same files stay selected after a restart.
    pub fn select_files(&self, id: &str, indexes: &Value) -> Result<Option<Value>, String> {
        let mut items = self.inner.items.lock().unwrap();
        let Some(item) = items.get_mut(id) else { return Ok(None) };
        let indexes = clean_indexes(indexes)?;
        let count = item.meta.files().len();
        if count == 0 {
            return Err("Dosya listesi henüz gelmedi.".into());
        }
        if indexes.iter().any(|i| *i >= count) {
            return Err("Geçersiz dosya seçimi.".into());
        }
        if !item.meta.allows_programs() && indexes.iter().any(|&i| file_kind(&item.meta.files()[i].name) == Kind::Program) {
            return Err("Çalıştırılabilir dosyalar (.exe gibi) güvenlik için indirilmez.".into());
        }
        self.inner.detach(item, false);
        item.meta.selected_indexes = if indexes.len() == count { None } else { Some(indexes) };
        item.meta.pick_files = None;
        item.meta.completed_at = None;
        self.inner.start_or_queue(&mut items, id, false);
        let item = items.get_mut(id).unwrap();
        if item.torrent.is_some() {
            item.meta.status = "downloading".into();
        }
        self.inner.save(&items);
        Ok(Some(Inner::snapshot(&items[id])))
    }

    pub fn pause(&self, id: &str) -> Option<Value> {
        let mut items = self.inner.items.lock().unwrap();
        let item = items.get_mut(id)?;
        if item.meta.status == "picking" {
            return Some(Inner::snapshot(item));
        }
        if item.torrent.is_some() {
            item.meta.progress = Some(Inner::progress_of(item).0);
        }
        Inner::cancel_retry(item);
        self.inner.detach(item, false);
        item.meta.status = "paused".into();
        self.inner.save(&items);
        self.inner.pump(&mut items);
        Some(Inner::snapshot(&items[id]))
    }

    pub fn resume(&self, id: &str) -> Option<Value> {
        let mut items = self.inner.items.lock().unwrap();
        let item = items.get_mut(id)?;
        // "Resume" or "Try again" starts at once, without waiting in the queue, with a fresh retry count.
        if item.torrent.is_none() && item.meta.status != "done" {
            Inner::cancel_retry(item);
            item.meta.retry_count = Some(0);
            let force = ["queued", "paused", "error", "connecting"].contains(&item.meta.status.as_str());
            self.inner.start_or_queue(&mut items, id, force);
        }
        self.inner.save(&items);
        Some(Inner::snapshot(&items[id]))
    }

    pub fn remove(&self, id: &str) -> bool {
        let mut items = self.inner.items.lock().unwrap();
        let Some(mut item) = items.shift_remove(id) else { return false };
        Inner::cancel_retry(&mut item);
        self.inner.detach(&mut item, true);
        self.inner.save(&items);
        self.inner.pump(&mut items);
        true
    }

    pub fn get(&self, id: &str) -> Option<Value> {
        self.inner.items.lock().unwrap().get(id).map(Inner::snapshot)
    }

    pub fn folder_of(&self, id: &str) -> Option<PathBuf> {
        let items = self.inner.items.lock().unwrap();
        let item = items.get(id)?;
        let name = item.torrent.as_ref().and_then(|t| t.state().name).or(item.meta.name.clone());
        let full = match name {
            Some(n) => Path::new(&item.meta.path).join(n),
            None => PathBuf::from(&item.meta.path),
        };
        if full.exists() {
            return Some(full);
        }
        let _ = std::fs::create_dir_all(&item.meta.path);
        Some(PathBuf::from(&item.meta.path))
    }

    /// A video or subtitle file: from disk when finished, otherwise streamed
    /// from the torrent (with the needed pieces fetched first).
    pub fn open_file(&self, id: &str, index: usize) -> Option<OpenedFile> {
        let items = self.inner.items.lock().unwrap();
        let item = items.get(id)?;
        let f = item.meta.files().get(index)?.clone();
        let mime = video_type(&f.name).unwrap_or("application/octet-stream").to_string();
        if let Some(t) = item.live() {
            if item.meta.status != "done" {
                if let Some(lf) = t.state().files.get(index) {
                    return Some(OpenedFile::Live { name: f.name, size: lf.length, mime, torrent: t.clone(), index });
                }
            }
        }
        let full = item.meta.file_path(&f);
        let size = std::fs::metadata(&full).ok()?.len();
        Some(OpenedFile::Disk { name: f.name, size, mime, path: full })
    }

    /// Best subtitle for a video: same-named .srt, otherwise a Turkish one, otherwise
    /// the first. Works before the download finishes: a complete file on disk is
    /// read; otherwise, while the torrent is open, the small subtitle is streamed first.
    pub async fn subtitle_for(&self, id: &str, index: usize) -> Option<(String, String)> {
        self.subtitle_for_with(id, index, SUBTITLE_WAIT).await
    }

    pub async fn subtitle_for_with(&self, id: &str, index: usize, wait: Duration) -> Option<(String, String)> {
        let (best, ready, path, live) = {
            let items = self.inner.items.lock().unwrap();
            let item = items.get(id)?;
            let files = item.meta.files();
            let video = files.get(index)?;
            let live = item.live().cloned();
            let lst = live.as_ref().map(|t| t.state());
            // qBittorrent creates the file at full size; while downloading the engine's state counts, not the size.
            let on_disk = |i: usize, f: &FileMeta| match lst.as_ref().and_then(|s| s.files.get(i)) {
                Some(lf) => lf.done(),
                None => std::fs::metadata(item.meta.file_path(f)).map(|m| m.len() == f.length).unwrap_or(false),
            };
            let base = video.name.rsplit_once('.').map(|(b, _)| b).unwrap_or(&video.name).to_lowercase();
            let tr = Regex::new(r"(^|[^a-z])(tr|tur|turkish|türkçe|turkce)([^a-z]|$)").unwrap();
            let mut subs: Vec<(f64, usize, bool)> = files
                .iter()
                .enumerate()
                .filter(|(_, f)| file_kind(&f.name) == Kind::Subtitle && f.length as usize <= MAX_SUBTITLE_BYTES)
                .map(|(i, f)| (i, f, on_disk(i, f)))
                .filter(|(i, _, ready)| *ready || lst.as_ref().is_some_and(|s| s.files.get(*i).is_some()))
                .map(|(i, f, ready)| {
                    let n = f.name.to_lowercase();
                    let score = if n.starts_with(&base) { 4.0 } else { 0.0 } + if tr.is_match(&n) { 2.0 } else { 0.0 } + if ready { 0.5 } else { 0.0 };
                    (score, i, ready)
                })
                .collect();
            subs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            let (_, best, ready) = *subs.first()?;
            (files[best].clone(), ready, item.meta.file_path(&files[best]), live)
        };
        if ready {
            let buf = std::fs::read(&path).ok()?;
            return Some((best.name.clone(), subtitle_to_vtt(&buf, &best.name)));
        }
        let t = live?;
        let idx = t.state().files.iter().position(|f| f.path == best.path)?;
        let mut rx = self.inner.engine.read_stream(&t, idx, 0, best.length.saturating_sub(1));
        let read = async {
            let mut buf = vec![];
            while let Some(chunk) = rx.recv().await {
                buf.extend_from_slice(&chunk.ok()?);
            }
            Some(buf)
        };
        let buf = tokio::time::timeout(wait, read).await.ok()??;
        Some((best.name.clone(), subtitle_to_vtt(&buf, &best.name)))
    }

    pub fn engine(&self) -> &Engine {
        &self.inner.engine
    }

    /// Re-check the queue when a setting changes (most downloads at once).
    pub fn pump(&self) {
        let mut items = self.inner.items.lock().unwrap();
        self.inner.pump(&mut items);
    }

    /// Bytes per second; 0 = unlimited.
    pub fn throttle(&self, down: u64, up: u64) {
        self.inner.engine.throttle(down, up);
    }

    pub async fn close(&self) {
        {
            let mut items = self.inner.items.lock().unwrap();
            for item in items.values_mut() {
                Inner::cancel_retry(item);
                let Some(t) = &item.torrent else { continue };
                if item.meta.status != "done" && item.meta.status != "picking" {
                    item.meta.progress = Some(Inner::progress_of(item).0);
                }
                item.meta.uploaded = Some(item.meta.uploaded.unwrap_or(0) + t.state().uploaded);
            }
            self.inner.save(&items);
        }
        self.inner.engine.destroy().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temporary_network_errors_are_told_apart() {
        assert!(is_transient_error("connect ETIMEDOUT 1.2.3.4:6881"));
        assert!(is_transient_error("fetch failed"));
        assert!(is_transient_error("Error downloading torrent: ECONNRESET"));
        assert!(!is_transient_error("Invalid torrent identifier"));
        assert!(!is_transient_error(BLOCKED_MESSAGE));
        assert_eq!(friendly_error("Invalid torrent identifier"), "Geçersiz torrent dosyası.");
    }

    #[test]
    fn old_download_list_entries_still_load() {
        let v = json!({"id": "x", "title": "Film", "magnet": null, "status": "done", "addedAt": 1, "files": [{"name": "a.mp4", "path": "F/a.mp4", "length": 3}], "eskiAlan": true});
        let m: Meta = serde_json::from_value(v).unwrap();
        assert_eq!(m.files().len(), 1);
        assert_eq!(serde_json::to_value(&m).unwrap()["eskiAlan"], true);
    }
}
