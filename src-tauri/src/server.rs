//! Local web server for the app's own page: the API, the media stream for the
//! player and the page files. It listens on 127.0.0.1 only, and every API call
//! needs the session key that is written into the page.

use crate::categories::{detect_genres, CATEGORIES, GENRES};
use crate::downloads::{Downloads, Hooks, OpenedFile};
use crate::files::{subtitle_to_vtt, MAX_SUBTITLE_BYTES};
use crate::library::Library;
use crate::model::SearchResult;
use crate::net::Net;
use crate::qbittorrent::Engine;
use crate::ratings::{add_imdb_ratings, RatingsCache, BUDGET};
use crate::search::{active_providers, search_all, SearchDeps, SearchParams};
use crate::settings::SettingsStore;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::Response;
use base64::Engine as _;
use rand::RngCore;
use rust_embed::RustEmbed;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MASK: &str = "••••••";

#[derive(RustEmbed)]
#[folder = "../public"]
struct Public;

/// The app's data folder. Settings, downloads and library saved under the old
/// name ("Torrent Bul") stay where they are.
pub fn default_data_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    let legacy = base.join("Torrent Bul");
    if legacy.exists() {
        return legacy;
    }
    base.join("Compass")
}

pub fn system_open(target: &Path) {
    if let Err(e) = open::that_detached(target) {
        eprintln!("[aç] {}: {e}", target.display());
    }
}

pub type Opener = Arc<dyn Fn(&Path) + Send + Sync>;

pub struct Options {
    pub port: u16,
    pub data_dir: PathBuf,
    pub open_path: Opener,
    pub net: Net,
    /// Replaces the qBittorrent engine (tests).
    pub engine: Option<(Engine, tokio::sync::mpsc::UnboundedReceiver<(u64, crate::qbittorrent::Event)>)>,
    pub retry_base: Duration,
}

impl Options {
    pub fn new(data_dir: PathBuf) -> Self {
        Options { port: 0, data_dir, open_path: Arc::new(|p: &Path| system_open(p)), net: Net::new(), engine: None, retry_base: Duration::from_secs(3) }
    }
}

struct AppState {
    settings: Arc<SettingsStore>,
    library: Library,
    ratings: Arc<RatingsCache>,
    downloads: Downloads,
    net: Net,
    token: String,
    open_path: Opener,
}

pub struct App {
    pub url: String,
    pub token: String,
    state: Arc<AppState>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl App {
    pub fn settings(&self) -> crate::settings::Settings {
        self.state.settings.get()
    }
    /// For the desktop app (e.g. a magnet link from the operating system).
    pub fn add_download(&self, body: &Value) -> Result<Value, String> {
        self.state.downloads.add(body)
    }
    pub async fn close(&mut self) {
        self.state.downloads.close().await;
        self.state.library.close();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

pub async fn start(opts: Options) -> std::io::Result<App> {
    let dir = &opts.data_dir;
    std::fs::create_dir_all(dir)?;
    let settings = Arc::new(SettingsStore::open(dir.join("settings.json")));
    let library = Library::open(dir.join("library.json"), crate::library::PROGRESS_SAVE);
    let ratings = Arc::new(RatingsCache::open(Some(dir.join("omdb-cache.json"))));
    let (engine, events) = opts.engine.unwrap_or_else(|| Engine::new(dir.join("qbittorrent"), opts.net.clone()));
    let (s1, s2, s3, lib) = (settings.clone(), settings.clone(), settings.clone(), library.clone());
    let downloads = Downloads::new(
        dir.join("downloads.json"),
        engine,
        events,
        Hooks {
            download_dir: Box::new(move || s1.get().download_dir),
            should_seed: Box::new(move || s2.get().seed_after_done),
            max_active: Box::new(move || s3.get().max_active_downloads as usize),
            on_done: Box::new(move |info| {
                lib.upsert(&info);
            }),
        },
        opts.retry_base,
    );
    let s = settings.get();
    downloads.throttle(s.download_limit_kbps * 1024, s.upload_limit_kbps * 1024);
    // Downloads finished in versions before the library get into it too.
    for info in downloads.completed() {
        if !library.has(info.info_hash.as_deref().unwrap_or_default()) {
            library.upsert(&info);
        }
    }
    // A session key so only this app's own page can call the API.
    let mut raw = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut raw);
    let token = crate::torrent::hex(&raw);
    let state = Arc::new(AppState { settings, library, ratings, downloads, net: opts.net, token: token.clone(), open_path: opts.open_path });

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", opts.port)).await?;
    let port = listener.local_addr()?.port();
    let router = axum::Router::new().fallback(handle).with_state(state.clone());
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).with_graceful_shutdown(async { rx.await.ok(); }).await;
    });
    Ok(App { url: format!("http://127.0.0.1:{port}/"), token, state, shutdown: Some(tx) })
}

fn send(status: u16, body: Value) -> Response {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn error(status: u16, msg: &str) -> Response {
    send(status, json!({"error": msg}))
}

async fn read_json(req: Request) -> Result<Value, String> {
    let bytes = axum::body::to_bytes(req.into_body(), 25_000_000).await.map_err(|_| "İstek çok büyük.".to_string())?;
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

async fn handle(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    if path.starts_with("/media/") {
        return serve_media(&st, req).await;
    }
    if !path.starts_with("/api/") {
        return serve_static(&st, &path);
    }
    if req.headers().get("x-app-token").and_then(|v| v.to_str().ok()) != Some(&st.token) {
        return error(403, "Yetkisiz istek.");
    }
    match api(&st, req).await {
        Ok(r) => r,
        Err(e) => error(400, if e.is_empty() { "Beklenmeyen hata." } else { &e }),
    }
}

fn public_settings(st: &AppState) -> Value {
    let s = st.settings.get();
    let mut v = serde_json::to_value(&s).unwrap();
    v["omdbApiKey"] = json!(if s.omdb_api_key.is_empty() { "" } else { MASK });
    v["torznab"] = json!(s
        .torznab
        .iter()
        .map(|t| {
            let mut x = serde_json::to_value(t).unwrap();
            x["apiKey"] = json!(if t.api_key.is_empty() { "" } else { MASK });
            x
        })
        .collect::<Vec<_>>());
    v
}

fn query(req: &Request) -> std::collections::HashMap<String, String> {
    url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect()
}

async fn api(st: &Arc<AppState>, req: Request) -> Result<Response, String> {
    let path = req.uri().path().to_string();
    let parts: Vec<String> =
        path.split('/').filter(|p| !p.is_empty()).map(|p| percent_encoding::percent_decode_str(p).decode_utf8_lossy().into_owned()).collect();
    let method = req.method().clone();
    let route = format!("{} /{}", method, parts[1..].join("/"));
    let p = |i: usize| parts.get(i).map(String::as_str);

    match route.as_str() {
        "GET /meta" => {
            let s = st.settings.get();
            return Ok(send(200, json!({
                "categories": CATEGORIES.iter().map(|c| (c.key.to_string(), json!(c.label))).collect::<serde_json::Map<_, _>>(),
                "genres": GENRES.iter().map(|g| (g.key.to_string(), json!(g.label))).collect::<serde_json::Map<_, _>>(),
                "imdb": !s.omdb_api_key.is_empty(),
                "providers": active_providers(&s).iter().map(|p| json!({"id": p.id(), "name": p.name()})).collect::<Vec<_>>(),
            })));
        }
        "GET /search" => {
            let q = query(&req);
            let params = SearchParams {
                query: q.get("q").map(|s| s.chars().take(200).collect()).unwrap_or_default(),
                category: q.get("category").filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "all".into()),
                genre: q.get("genre").cloned().unwrap_or_default(),
            };
            if params.query.trim().is_empty() && params.genre.is_empty() && params.category != "adult" {
                return Ok(error(400, "Bir şey yazın ya da bir tür seçin."));
            }
            let s = st.settings.get();
            let providers = active_providers(&s);
            if providers.is_empty() {
                return Ok(error(400, "Etkin arama kaynağı yok. Ayarlar’dan bir kaynak açın."));
            }
            let out = search_all(&params, &providers, SearchDeps { net: &st.net, omdb_api_key: &s.omdb_api_key, cache: &st.ratings, timeout: None, ratings_budget: BUDGET }).await;
            return Ok(send(200, serde_json::to_value(out).unwrap()));
        }
        // Posters and ratings that did not make it in time: the page asks after showing results.
        "POST /ratings" => {
            let body = read_json(req).await?;
            let tt = regex::Regex::new(r"^tt\d{5,10}$").unwrap();
            let mut items: Vec<SearchResult> = body["items"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(40)
                .filter(|r| r["id"].is_string() && r["title"].is_string() && matches!(r["category"].as_str(), Some("movies" | "tv")))
                .map(|r| SearchResult {
                    id: r["id"].as_str().unwrap().into(),
                    title: r["title"].as_str().unwrap().chars().take(300).collect(),
                    year: r["year"].as_i64().map(|y| y as i32),
                    category: r["category"].as_str().unwrap().into(),
                    imdb_id: r["imdbId"].as_str().filter(|s| tt.is_match(s)).map(String::from),
                    ..Default::default()
                })
                .collect();
            add_imdb_ratings(&mut items, &st.settings.get().omdb_api_key, &st.net, &st.ratings, Duration::from_secs(8)).await;
            let mut out = serde_json::Map::new();
            for r in items {
                if let Some(url) = &r.imdb_url {
                    out.insert(r.id.clone(), json!({"rating": r.rating, "poster": r.poster, "imdbUrl": url, "year": r.year, "genres": detect_genres(&r.subjects)}));
                }
            }
            return Ok(send(200, json!({"ratings": out})));
        }
        "GET /settings" => return Ok(send(200, public_settings(st))),
        "PUT /settings" => {
            let mut body = read_json(req).await?;
            // A masked key coming back keeps the old one.
            if let Some(list) = body.get_mut("torznab").and_then(Value::as_array_mut) {
                let old = st.settings.get().torznab;
                for t in list.iter_mut() {
                    if t["apiKey"] == MASK {
                        let prev = old.iter().find(|o| Some(o.url.as_str()) == t["url"].as_str()).map(|o| o.api_key.clone()).unwrap_or_default();
                        t["apiKey"] = json!(prev);
                    }
                }
            }
            if body["omdbApiKey"] == MASK {
                body.as_object_mut().map(|o| o.remove("omdbApiKey"));
            }
            let seed_before = st.settings.get().seed_after_done;
            let s = st.settings.update(&body).map_err(|e| e.to_string())?;
            if s.seed_after_done != seed_before {
                st.downloads.set_seeding(s.seed_after_done);
            }
            st.downloads.throttle(s.download_limit_kbps * 1024, s.upload_limit_kbps * 1024);
            st.downloads.pump();
            return Ok(send(200, public_settings(st)));
        }
        "GET /downloads" => return Ok(send(200, json!(st.downloads.list()))),
        "GET /library" => return Ok(send(200, json!(st.library.list()))),
        // Turns a subtitle the user picked into VTT only; nothing is written to disk.
        "POST /subtitles/preview" => {
            let body = read_json(req).await?;
            let name = body["name"].as_str().filter(|s| !s.is_empty()).unwrap_or("altyazi.srt").to_string();
            let buf = base64::engine::general_purpose::STANDARD.decode(body["data"].as_str().unwrap_or("")).unwrap_or_default();
            let ext = crate::files::ext(&name);
            if ext != ".srt" && ext != ".vtt" {
                return Ok(error(400, "Yalnızca .srt ya da .vtt dosyası."));
            }
            if buf.is_empty() {
                return Ok(error(400, "Altyazı dosyası boş."));
            }
            if buf.len() > MAX_SUBTITLE_BYTES {
                return Ok(error(400, "Altyazı dosyası 1 MB’tan büyük olamaz."));
            }
            return Ok(send(200, json!({"name": name, "vtt": subtitle_to_vtt(&buf, &name)})));
        }
        "POST /downloads" => {
            let body = read_json(req).await?;
            return Ok(send(201, st.downloads.add(&body)?));
        }
        "POST /torrents/inspect" => {
            let body = read_json(req).await?;
            let out: Vec<Value> = body["files"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .take(500)
                .map(|f| {
                    let file = f["name"].as_str().unwrap_or_default().to_string();
                    match st.downloads.inspect(f["data"].as_str().unwrap_or_default()) {
                        Ok(i) => {
                            let mut v = i.json;
                            v["file"] = json!(file);
                            v
                        }
                        Err(e) => json!({"file": file, "error": e}),
                    }
                })
                .collect();
            return Ok(send(200, json!(out)));
        }
        "POST /open-download-dir" => {
            let dir = PathBuf::from(st.settings.get().download_dir);
            let _ = std::fs::create_dir_all(&dir);
            (st.open_path)(&dir);
            return Ok(send(200, json!({"ok": true})));
        }
        _ => {}
    }

    if method == Method::PUT && p(1) == Some("library") && p(3) == Some("progress") && p(4).is_none() {
        let hash = p(2).unwrap_or_default().to_string();
        let body = read_json(req).await?;
        return Ok(match st.library.progress(&hash, &body) {
            Some(v) => send(200, v),
            None => error(404, "Kütüphanede yok."),
        });
    }
    if p(1) == Some("downloads") && p(2).is_some() {
        let id = p(2).unwrap().to_string();
        let action = p(3).map(String::from);
        let out: Option<Value> = match (method.clone(), action.as_deref()) {
            (Method::GET, Some("files")) => st.downloads.files(&id),
            (Method::POST, Some("files")) => {
                let body = read_json(req).await?;
                st.downloads.select_files(&id, &body["indexes"])?
            }
            (Method::POST, Some("pause")) => st.downloads.pause(&id),
            (Method::POST, Some("resume")) => st.downloads.resume(&id),
            (Method::POST, Some("open-file")) => {
                let body = read_json(req).await?;
                let index = body["index"].as_u64().map(|n| n as usize);
                match index.and_then(|i| st.downloads.open_file(&id, i)) {
                    Some(OpenedFile::Disk { path, .. }) => {
                        (st.open_path)(&path);
                        Some(json!({"ok": true}))
                    }
                    _ => None,
                }
            }
            (Method::POST, Some("open")) => st.downloads.folder_of(&id).map(|folder| {
                (st.open_path)(&folder);
                json!({"ok": true})
            }),
            (Method::DELETE, None) => st.downloads.remove(&id).then(|| json!({"ok": true})),
            _ => return Ok(error(405, "Desteklenmeyen işlem.")),
        };
        return Ok(match out {
            Some(v) => send(200, v),
            None => error(404, "İndirme bulunamadı."),
        });
    }
    Ok(error(404, "Bulunamadı."))
}

fn mime(path: &str) -> &'static str {
    match crate::files::ext(path).as_str() {
        ".html" => "text/html; charset=utf-8",
        ".css" => "text/css; charset=utf-8",
        ".js" => "text/javascript; charset=utf-8",
        ".svg" => "image/svg+xml",
        ".png" => "image/png",
        ".ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn serve_static(st: &AppState, path: &str) -> Response {
    let rel = if path == "/" { "index.html".to_string() } else { percent_encoding::percent_decode_str(path).decode_utf8_lossy().trim_start_matches('/').to_string() };
    if rel.split(['/', '\\']).any(|seg| seg == "..") {
        return error(403, "Yasak.");
    }
    let Some(file) = Public::get(&rel) else { return error(404, "Bulunamadı.") };
    let mut body = file.data.into_owned();
    if rel == "index.html" {
        body = String::from_utf8_lossy(&body).replace("__APP_TOKEN__", &st.token).into_bytes();
    }
    Response::builder()
        .header(header::CONTENT_TYPE, mime(&rel))
        .header(header::CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .body(Body::from(body))
        .unwrap()
}

/// Range header → (start, end), None for no range, Err for an unsatisfiable one.
fn parse_range(headers: &HeaderMap, size: u64) -> Result<Option<(u64, u64)>, ()> {
    let Some(v) = headers.get(header::RANGE).and_then(|v| v.to_str().ok()) else { return Ok(None) };
    let re = regex::Regex::new(r"^bytes=(\d*)-(\d*)$").unwrap();
    let Some(c) = re.captures(v) else { return Ok(None) };
    let (a, b) = (&c[1], &c[2]);
    if a.is_empty() && b.is_empty() {
        return Ok(None);
    }
    let last = size.saturating_sub(1);
    let (start, end) = if !a.is_empty() {
        let s: u64 = a.parse().map_err(|_| ())?;
        let e = if b.is_empty() { last } else { b.parse::<u64>().map_err(|_| ())?.min(last) };
        (s, e)
    } else {
        // "bytes=-500": the last 500 bytes
        (size.saturating_sub(b.parse().map_err(|_| ())?), last)
    };
    if start > end || start >= size {
        return Err(());
    }
    Ok(Some((start, end)))
}

/// `<video>` and `<track>` cannot send custom headers, so the key travels in the address.
async fn serve_media(st: &AppState, req: Request) -> Response {
    let q = query(&req);
    if q.get("t") != Some(&st.token) {
        return error(403, "Yetkisiz istek.");
    }
    let parts: Vec<String> = req.uri().path().split('/').map(|p| percent_encoding::percent_decode_str(p).decode_utf8_lossy().into_owned()).collect();
    // /media/:id/:index[/subtitle.vtt]
    let id = parts.get(2).cloned().unwrap_or_default();
    let Some(index) = parts.get(3).and_then(|s| s.parse::<usize>().ok()) else { return error(404, "Dosya bulunamadı.") };
    if parts.get(4).map(String::as_str) == Some("subtitle.vtt") {
        let sub = if id.starts_with("lib-") { None } else { st.downloads.subtitle_for(&id, index).await };
        return match sub {
            Some((_, vtt)) => Response::builder()
                .header(header::CONTENT_TYPE, "text/vtt; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(vtt))
                .unwrap(),
            None => error(404, "Altyazı yok."),
        };
    }
    // "lib-<infohash>": the download was removed from the list but the file is in the library.
    let opened = if let Some(hash) = id.strip_prefix("lib-") {
        st.library.open_file(hash, index).map(|f| OpenedFile::Disk { name: f.name, size: f.size, mime: f.mime, path: f.path })
    } else {
        st.downloads.open_file(&id, index)
    };
    let Some(file) = opened else { return error(404, "Dosya bulunamadı. Taşınmış ya da silinmiş olabilir.") };
    let (size, mime) = match &file {
        OpenedFile::Disk { size, mime, .. } | OpenedFile::Live { size, mime, .. } => (*size, mime.clone()),
    };
    let range = match parse_range(req.headers(), size) {
        Ok(r) => r,
        Err(()) => return Response::builder().status(416).header(header::CONTENT_RANGE, format!("bytes */{size}")).body(Body::empty()).unwrap(),
    };
    let (start, end) = range.unwrap_or((0, size.saturating_sub(1)));
    let mut res = Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::CONTENT_LENGTH, if size == 0 { 0 } else { end - start + 1 });
    if range.is_some() {
        res = res.status(StatusCode::PARTIAL_CONTENT).header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{size}"));
    }
    if req.method() == Method::HEAD || size == 0 {
        return res.body(Body::empty()).unwrap();
    }
    let body = match file {
        OpenedFile::Disk { path, .. } => {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let Ok(mut f) = tokio::fs::File::open(&path).await else { return error(404, "Dosya bulunamadı.") };
            if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
                return error(404, "Dosya bulunamadı.");
            }
            Body::from_stream(tokio_util_stream(f.take(end - start + 1)))
        }
        OpenedFile::Live { torrent, index, .. } => {
            let rx = st.downloads.engine().read_stream(&torrent, index, start, end);
            Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx))
        }
    };
    res.body(body).unwrap()
}

/// Reads a file in 64 KB pieces as a stream.
fn tokio_util_stream<R: tokio::io::AsyncRead + Unpin + Send + 'static>(reader: R) -> impl futures::Stream<Item = std::io::Result<bytes::Bytes>> {
    futures::stream::unfold(reader, |mut r| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 64 * 1024];
        match r.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(bytes::Bytes::from(buf)), r))
            }
            Err(e) => Some((Err(e), r)),
        }
    })
}
