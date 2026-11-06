//! End-to-end tests with a real qbittorrent-nox: a local "Internet Archive"
//! serves the files over HTTP as a web seed. Skipped when qbittorrent-nox is missing.

use crate::net::testserver;
use crate::qbittorrent::find_qbittorrent;
use crate::server::{start, App, Options};
use crate::torrent::make_torrent;
use axum::body::Body;
use axum::response::Response;
use base64::Engine as _;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn fill(n: usize, k: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * k) % 251) as u8).collect()
}

struct Fixture {
    base: String,
    torrent: Vec<u8>,
    url: String,
    hold: Arc<AtomicBool>,
}

/// Serves `/item.torrent` and the files under `/<name>/<file>` with Range support.
/// While `hold` is set, file requests get 503 (nothing downloads).
async fn fixture(name: &str, files: &[(&str, Vec<u8>)]) -> Fixture {
    let map: HashMap<String, Vec<u8>> = files
        .iter()
        .map(|(f, b)| (if files.len() == 1 && *f == name { format!("/{f}") } else { format!("/{name}/{f}") }, b.clone()))
        .collect();
    let hold = Arc::new(AtomicBool::new(false));
    let torrent_cell: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(vec![]));
    let (h, tc) = (hold.clone(), torrent_cell.clone());
    let server = testserver::start(move |req| {
        let path = percent_encoding::percent_decode_str(req.uri().path()).decode_utf8_lossy().into_owned();
        if path == "/item.torrent" {
            return Response::new(Body::from(tc.lock().unwrap().clone()));
        }
        if h.load(Ordering::SeqCst) {
            return Response::builder().status(503).body(Body::empty()).unwrap();
        }
        let Some(body) = map.get(&path) else { return Response::builder().status(404).body(Body::empty()).unwrap() };
        let range = req.headers().get("range").and_then(|v| v.to_str().ok()).unwrap_or("");
        let re = regex::Regex::new(r"bytes=(\d+)-(\d+)?").unwrap();
        match re.captures(range) {
            Some(c) => {
                let start: usize = c[1].parse().unwrap();
                let end: usize = c.get(2).map(|m| m.as_str().parse().unwrap()).unwrap_or(body.len() - 1).min(body.len() - 1);
                Response::builder()
                    .status(206)
                    .header("content-range", format!("bytes {start}-{end}/{}", body.len()))
                    .header("accept-ranges", "bytes")
                    .body(Body::from(body[start..=end].to_vec()))
                    .unwrap()
            }
            None => Response::builder().header("accept-ranges", "bytes").body(Body::from(body.clone())).unwrap(),
        }
    })
    .await;
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(f, b)| (*f, b.as_slice())).collect();
    let torrent = make_torrent(name, &refs, 16384, Some(&format!("{}/", server.base)));
    *torrent_cell.lock().unwrap() = torrent.clone();
    Fixture { url: format!("{}/item.torrent", server.base), base: server.base, torrent, hold }
}

struct Client {
    http: reqwest::Client,
    app: App,
}

impl Client {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let url = format!("{}api/{}", self.app.url, path);
        let mut req = self.http.request(method.parse().unwrap(), url).header("X-App-Token", &self.app.token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        (status, res.json().await.unwrap_or(Value::Null))
    }

    async fn wait_for(&self, check: impl Fn(&Vec<Value>) -> bool) -> Vec<Value> {
        let mut list = vec![];
        for _ in 0..150 {
            list = self.call("GET", "downloads", None).await.1.as_array().cloned().unwrap_or_default();
            if check(&list) {
                return list;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("zaman aşımı; son durum: {}", json!(list));
    }
}

async fn open_app(data: &Path, opened: Arc<Mutex<Vec<PathBuf>>>) -> Client {
    let mut opts = Options::new(data.to_path_buf());
    opts.open_path = Arc::new(move |p: &Path| opened.lock().unwrap().push(p.to_path_buf()));
    Client { http: reqwest::Client::builder().no_proxy().build().unwrap(), app: start(opts).await.unwrap() }
}

fn on_disk(file: &Path, content: &[u8]) -> bool {
    std::fs::read(file).map(|b| b == content).unwrap_or(false)
}

macro_rules! need_qbittorrent {
    () => {
        if find_qbittorrent().is_none() {
            eprintln!("qbittorrent-nox kurulu değil; atlandı");
            return;
        }
    };
}

#[tokio::test(flavor = "multi_thread")]
async fn api_rejects_requests_without_key_and_page_carries_the_key() {
    let dir = tempfile::tempdir().unwrap();
    let c = open_app(dir.path(), Default::default()).await;
    let res = c.http.get(format!("{}api/downloads", c.app.url)).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 403);
    let html = c.http.get(&c.app.url).send().await.unwrap().text().await.unwrap();
    assert!(html.contains(&c.app.token));
    let traversal = c.http.get(format!("{}..%2fCargo.toml", c.app.url)).send().await.unwrap();
    assert_ne!(traversal.status().as_u16(), 200);
    let media = c.http.get(format!("{}media/x/0?t=wrong", c.app.url)).send().await.unwrap();
    assert_eq!(media.status().as_u16(), 403);
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_are_saved_and_api_keys_are_masked() {
    let dir = tempfile::tempdir().unwrap();
    let c = open_app(dir.path(), Default::default()).await;
    let (_, put) = c
        .call("PUT", "settings", Some(json!({"downloadDir": "/tmp/x", "torznab": [{"name": "J", "url": "http://localhost:9117/torznab", "apiKey": "secret"}]})))
        .await;
    assert_eq!(put["torznab"][0]["apiKey"], "••••••");
    // Saving again with the masked key keeps the real key.
    c.call("PUT", "settings", Some(json!({"torznab": put["torznab"]}))).await;
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("settings.json")).unwrap()).unwrap();
    assert_eq!(saved["torznab"][0]["apiKey"], "secret");
    let (_, meta) = c.call("GET", "meta", None).await;
    let names: Vec<&str> = meta["providers"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Internet Archive", "J"]);
    let (status, _) = c.call("POST", "downloads", Some(json!({"magnet": "http://not-a-magnet"}))).await;
    assert_eq!(status, 400, "invalid magnet is refused");
}

#[tokio::test(flavor = "multi_thread")]
async fn local_subtitle_preview_returns_vtt_and_refuses_big_files() {
    let dir = tempfile::tempdir().unwrap();
    let c = open_app(dir.path(), Default::default()).await;
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    let (s, v) = c.call("POST", "subtitles/preview", Some(json!({"name": "a.srt", "data": b64(b"1\n00:00:01,000 --> 00:00:02,000\nMerhaba\n")}))).await;
    assert_eq!(s, 200);
    assert!(v["vtt"].as_str().unwrap().starts_with("WEBVTT\n\n1\n00:00:01.000"));
    let (s, _) = c.call("POST", "subtitles/preview", Some(json!({"name": "a.srt", "data": b64(&vec![b'a'; 1024 * 1024 + 1])}))).await;
    assert_eq!(s, 400);
    let (s, _) = c.call("POST", "subtitles/preview", Some(json!({"name": "a.exe", "data": b64(b"x")}))).await;
    assert_eq!(s, 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn web_seed_torrent_downloads_end_to_end_like_internet_archive() {
    need_qbittorrent!();
    let content = fill(512 * 1024 + 123, 31);
    let fx = fixture("ornek-film.bin", &[("ornek-film.bin", content.clone())]).await;
    let dir = tempfile::tempdir().unwrap();
    let dl = dir.path().join("indirilenler");
    let opened: Arc<Mutex<Vec<PathBuf>>> = Default::default();
    let mut c = open_app(dir.path(), opened.clone()).await;
    c.call("PUT", "settings", Some(json!({"downloadDir": dl}))).await;
    let (status, added) = c.call("POST", "downloads", Some(json!({"torrentUrl": fx.url, "title": "Örnek Film", "source": "Internet Archive"}))).await;
    assert_eq!(status, 201, "{added}");
    let (_, again) = c.call("POST", "downloads", Some(json!({"torrentUrl": fx.url}))).await;
    assert_eq!(again["id"], added["id"], "same torrent is not added twice");
    let list = c.wait_for(|l| l.first().is_some_and(|d| d["status"] == "done")).await;
    assert_eq!(list[0]["progress"], 1.0);
    assert!(on_disk(&dl.join("ornek-film.bin"), &content));
    // The .torrent we fetched is kept (not qBittorrent's export, which loses the web seed).
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("downloads.json")).unwrap()).unwrap();
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(saved[0]["torrentFile"].as_str().unwrap()).unwrap(), fx.torrent);
    let id = list[0]["id"].as_str().unwrap().to_string();
    let (s, _) = c.call("POST", &format!("downloads/{id}/open"), None).await;
    assert_eq!(s, 200);
    assert_eq!(opened.lock().unwrap()[0], dl.join("ornek-film.bin"));
    let (s, _) = c.call("DELETE", &format!("downloads/{id}"), None).await;
    assert_eq!(s, 200);
    assert!(c.call("GET", "downloads", None).await.1.as_array().unwrap().is_empty());
    assert!(dl.join("ornek-film.bin").exists(), "removing from the list keeps the file");
    let _ = fx.base;
    c.app.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_chosen_files_download_and_the_choice_survives_a_restart() {
    need_qbittorrent!();
    let film = fill(128 * 1024, 7);
    let sample = fill(64 * 1024, 11);
    let srt = b"1\n00:00:01,000 --> 00:00:02,000\nMerhaba\n".to_vec();
    let fx = fixture("Paket", &[("film.mp4", film.clone()), ("sample.mp4", sample.clone()), ("film.tr.srt", srt.clone())]).await;
    let dir = tempfile::tempdir().unwrap();
    let dl = dir.path().join("dl");
    let mut c = open_app(dir.path(), Default::default()).await;
    c.call("PUT", "settings", Some(json!({"downloadDir": dl}))).await;
    let (_, added) = c.call("POST", "downloads", Some(json!({"torrentUrl": fx.url, "title": "Paket"}))).await;
    let id = added["id"].as_str().unwrap().to_string();
    let list = c.wait_for(|l| l.first().is_some_and(|d| d["status"] == "picking")).await;
    assert_eq!(list[0]["fileCount"], 3);
    let (s, files) = c.call("GET", &format!("downloads/{id}/files"), None).await;
    assert_eq!(s, 200);
    let rows: Vec<(String, String, u64)> = files["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["name"].as_str().unwrap().into(), f["kind"].as_str().unwrap().into(), f["length"].as_u64().unwrap()))
        .collect();
    assert_eq!(rows, [("film.mp4".into(), "video".into(), film.len() as u64), ("sample.mp4".into(), "video".into(), sample.len() as u64), ("film.tr.srt".into(), "subtitle".into(), 40)]);
    // Nothing downloads while waiting for the choice.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!on_disk(&dl.join("Paket/film.mp4"), &film));
    assert_eq!(c.call("POST", &format!("downloads/{id}/files"), Some(json!({"indexes": []}))).await.0, 400);
    assert_eq!(c.call("POST", &format!("downloads/{id}/files"), Some(json!({"indexes": [9]}))).await.0, 400);
    let (s, chosen) = c.call("POST", &format!("downloads/{id}/files"), Some(json!({"indexes": [0, 2]}))).await;
    assert_eq!(s, 200);
    assert_eq!(chosen["status"], "downloading");
    let list = c.wait_for(|l| l.first().is_some_and(|d| d["status"] == "done")).await;
    assert_eq!(list[0]["missing"], false);
    assert_eq!(list[0]["videos"].as_array().unwrap().iter().map(|v| v["name"].as_str().unwrap()).collect::<Vec<_>>(), ["film.mp4"]);
    assert!(on_disk(&dl.join("Paket/film.mp4"), &film));
    assert!(!on_disk(&dl.join("Paket/sample.mp4"), &sample), "file that was not chosen did not download");

    // The finished film is in the library and plays from the media address with Range.
    // The list can say "done" (qBittorrent's state) a moment before the library entry is written.
    let mut lib = Value::Null;
    for _ in 0..50 {
        lib = c.call("GET", "library", None).await.1;
        if lib.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(lib[0]["items"][0]["name"], "film.mp4");
    let media = format!("{}media/{id}/0?t={}", c.app.url, c.app.token);
    let res = c.http.get(&media).header("range", "bytes=10-19").send().await.unwrap();
    assert_eq!(res.status().as_u16(), 206);
    assert_eq!(res.bytes().await.unwrap().to_vec(), film[10..20].to_vec());
    let sub = c.http.get(format!("{}media/{id}/0/subtitle.vtt?t={}", c.app.url, c.app.token)).send().await.unwrap();
    assert!(sub.text().await.unwrap().contains("Merhaba"));

    let saved: Value = serde_json::from_str(&std::fs::read_to_string(dir.path().join("downloads.json")).unwrap()).unwrap();
    assert_eq!(saved[0]["selectedIndexes"], json!([0, 2]));
    c.app.close().await;
    let mut c = open_app(dir.path(), Default::default()).await;
    let (_, again) = c.call("GET", &format!("downloads/{id}/files"), None).await;
    assert_eq!(again["files"].as_array().unwrap().iter().map(|f| f["selected"].as_bool().unwrap()).collect::<Vec<_>>(), [true, false, true]);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(c.call("GET", "downloads", None).await.1[0]["status"], "done");
    assert!(!on_disk(&dl.join("Paket/sample.mp4"), &sample));
    c.app.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn interrupted_partial_download_resumes_with_only_the_chosen_file() {
    need_qbittorrent!();
    let a = fill(64 * 1024, 3);
    let b = fill(96 * 1024, 5);
    let fx = fixture("Dizi", &[("bolum1.mkv", a.clone()), ("bolum2.mkv", b.clone())]).await;
    let dir = tempfile::tempdir().unwrap();
    let dl = dir.path().join("dl");
    let mut c = open_app(dir.path(), Default::default()).await;
    c.call("PUT", "settings", Some(json!({"downloadDir": dl}))).await;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&fx.torrent);
    let (_, inspected) = c.call("POST", "torrents/inspect", Some(json!({"files": [{"name": "dizi.torrent", "data": b64}]}))).await;
    assert_eq!(inspected[0]["files"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap()).collect::<Vec<_>>(), ["video", "video"]);
    fx.hold.store(true, Ordering::SeqCst);
    let (s, added) = c.call("POST", "downloads", Some(json!({"torrentFile": b64, "fileIndexes": [1]}))).await;
    assert_eq!(s, 201);
    assert_ne!(added["status"], "picking");
    c.app.close().await;
    let mut c = open_app(dir.path(), Default::default()).await;
    fx.hold.store(false, Ordering::SeqCst);
    let list = c.wait_for(|l| l.first().is_some_and(|d| d["status"] == "done")).await;
    assert_eq!(list[0]["length"], b.len());
    assert!(on_disk(&dl.join("Dizi/bolum2.mkv"), &b));
    assert!(!on_disk(&dl.join("Dizi/bolum1.mkv"), &a), "episode that was not chosen did not download");
    c.app.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn exe_posing_as_a_film_never_downloads_and_an_exe_next_to_a_film_is_skipped() {
    need_qbittorrent!();
    let exe = vec![7u8; 64 * 1024];
    let fake = fixture("Film.2024", &[("Film.2024.1080p.mp4.exe", exe.clone()), ("Okubeni.txt", b"x".to_vec())]).await;
    let dir = tempfile::tempdir().unwrap();
    let dl = dir.path().join("dl");
    let mut c = open_app(dir.path(), Default::default()).await;
    c.call("PUT", "settings", Some(json!({"downloadDir": dl}))).await;
    c.call("POST", "downloads", Some(json!({"torrentUrl": fake.url}))).await;
    let list = c.wait_for(|l| l.first().is_some_and(|d| d["status"] == "error")).await;
    assert_eq!(list[0]["error"], crate::files::BLOCKED_MESSAGE);
    assert!(!on_disk(&dl.join("Film.2024/Film.2024.1080p.mp4.exe"), &exe));
    // As a .torrent file it is refused before it is added.
    let other = fixture("Film.2025", &[("Film.2025.mp4.exe", vec![8u8; 1024]), ("Okubeni.txt", b"y".to_vec())]).await;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&other.torrent);
    assert_eq!(c.call("POST", "downloads", Some(json!({"torrentFile": b64}))).await.0, 400);
    // In the Software category programs are allowed.
    let (s, _) = c.call("POST", "downloads", Some(json!({"torrentFile": b64, "category": "software"}))).await;
    assert_eq!(s, 201);

    let film = fill(80 * 1024, 9);
    let mixed = fixture("Gercek", &[("film.mp4", film.clone()), ("codec.exe", exe.clone())]).await;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&mixed.torrent);
    let (s, added) = c.call("POST", "downloads", Some(json!({"torrentFile": b64}))).await;
    assert_eq!(s, 201);
    assert_eq!(added["skippedPrograms"], 1);
    let id = added["id"].as_str().unwrap().to_string();
    let (s, _) = c.call("POST", &format!("downloads/{id}/files"), Some(json!({"indexes": [0, 1]}))).await;
    assert_eq!(s, 400, "a program cannot be chosen");
    c.wait_for(|l| l.iter().any(|d| d["id"] == id.as_str() && d["status"] == "done")).await;
    assert!(on_disk(&dl.join("Gercek/film.mp4"), &film));
    assert!(!on_disk(&dl.join("Gercek/codec.exe"), &exe));
    c.app.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn finished_download_closes_and_a_moved_file_is_not_downloaded_again() {
    need_qbittorrent!();
    let video = fill(300 * 1024, 7);
    let srt = b"1\n00:00:01,000 --> 00:00:02,000\nMerhaba\n".to_vec();
    let fx = fixture("Film", &[("film.mp4", video.clone()), ("film.tr.srt", srt)]).await;
    let dir = tempfile::tempdir().unwrap();
    let dl = dir.path().join("dl");
    let mut c = open_app(dir.path(), Default::default()).await;
    c.call("PUT", "settings", Some(json!({"downloadDir": dl}))).await;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&fx.torrent);
    c.call("POST", "downloads", Some(json!({"torrentFile": b64}))).await;
    let list = c.wait_for(|l| l.first().is_some_and(|d| d["status"] == "done")).await;
    assert_eq!(list[0]["seeding"], false, "sharing is off by default");
    std::fs::rename(dl.join("Film"), dir.path().join("moved")).unwrap();
    c.app.close().await;
    let mut c = open_app(dir.path(), Default::default()).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let list = c.call("GET", "downloads", None).await.1;
    assert_eq!(list[0]["status"], "done");
    assert_eq!(list[0]["missing"], true);
    assert!(!dl.join("Film/film.mp4").exists(), "moved file is not downloaded again");
    c.app.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_limit_one_keeps_the_second_download_waiting_until_the_first_ends() {
    need_qbittorrent!();
    let a = fixture("A", &[("A", fill(64 * 1024, 3))]).await;
    let b = fixture("B", &[("B", fill(64 * 1024, 5))]).await;
    a.hold.store(true, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let mut c = open_app(dir.path(), Default::default()).await;
    c.call("PUT", "settings", Some(json!({"downloadDir": dir.path().join("dl"), "maxActiveDownloads": 1}))).await;
    let ab64 = base64::engine::general_purpose::STANDARD.encode(&a.torrent);
    let bb64 = base64::engine::general_purpose::STANDARD.encode(&b.torrent);
    let (_, first) = c.call("POST", "downloads", Some(json!({"torrentFile": ab64}))).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let (_, second) = c.call("POST", "downloads", Some(json!({"torrentFile": bb64}))).await;
    assert_eq!(second["status"], "queued");
    // Pausing frees the place; the queued one starts and finishes.
    c.call("POST", &format!("downloads/{}/pause", first["id"].as_str().unwrap()), None).await;
    let sid = second["id"].as_str().unwrap().to_string();
    c.wait_for(|l| l.iter().any(|d| d["id"] == sid.as_str() && d["status"] == "done")).await;
    // Resuming the paused one goes on without being added to qBittorrent again.
    a.hold.store(false, Ordering::SeqCst);
    c.call("POST", &format!("downloads/{}/resume", first["id"].as_str().unwrap()), None).await;
    let fid = first["id"].as_str().unwrap().to_string();
    c.wait_for(|l| l.iter().any(|d| d["id"] == fid.as_str() && d["status"] == "done")).await;
    c.app.close().await;
}
