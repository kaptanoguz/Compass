//! qBittorrent download engine. The app starts qBittorrent with its own separate
//! profile (the user's own qBittorrent settings and list are never touched),
//! talks to it over the Web API and reports each torrent's progress back to
//! the download manager as events (metadata / file done / done / error).

use crate::net::Net;
use crate::torrent::{magnet_info_hash, parse_torrent};
use base64::Engine as _;
use futures::future::BoxFuture;
use rand::RngCore;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Kept under the old name so earlier downloads are found in the same category.
const CATEGORY: &str = "torrent-bul";
const NO_ETA: i64 = 8_640_000; // qBittorrent's "infinite" time value
const BUSY: &[&str] = &["metaDL", "checkingDL", "checkingUP", "checkingResumeData", "allocating", "moving"];

pub const NOT_FOUND_MESSAGE: &str =
    "qBittorrent bulunamadı. Terminalde \"sudo apt install qbittorrent-nox\" ile kurup uygulamayı yeniden aç.";

fn is_subtitle(name: &str) -> bool {
    matches!(crate::files::ext(name).as_str(), ".srt" | ".vtt" | ".ass" | ".ssa" | ".sub")
}

fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata().map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// qbittorrent-nox (no window) is tried first; otherwise the desktop version (minimised to the tray).
pub fn find_qbittorrent() -> Option<(PathBuf, bool)> {
    if let Ok(bin) = std::env::var("QBITTORRENT_BIN") {
        if !bin.is_empty() {
            let gui = !bin.contains("nox");
            return Some((PathBuf::from(bin), gui));
        }
    }
    let mut candidates: Vec<(PathBuf, bool)> = vec![];
    if cfg!(windows) {
        candidates.push((PathBuf::from(r"C:\Program Files\qBittorrent\qbittorrent.exe"), true));
        candidates.push((PathBuf::from(r"C:\Program Files (x86)\qBittorrent\qbittorrent.exe"), true));
    } else if cfg!(target_os = "macos") {
        candidates.push((PathBuf::from("/Applications/qBittorrent.app/Contents/MacOS/qbittorrent"), true));
    } else {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<PathBuf> = std::env::split_paths(&path).filter(|p| !p.as_os_str().is_empty()).collect();
        candidates.extend(dirs.iter().map(|d| (d.join("qbittorrent-nox"), false)));
        candidates.extend(dirs.iter().map(|d| (d.join("qbittorrent"), true)));
        candidates.push((PathBuf::from("/var/lib/flatpak/exports/bin/org.qbittorrent.qBittorrent"), true));
        if let Some(home) = dirs::home_dir() {
            candidates.push((home.join(".local/share/flatpak/exports/bin/org.qbittorrent.qBittorrent"), true));
        }
        candidates.push((PathBuf::from("/snap/bin/qbittorrent-arnatious"), true));
    }
    candidates.into_iter().find(|(p, _)| is_executable(p))
}

fn free_port() -> std::io::Result<u16> {
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

/// The Web UI password is stored as PBKDF2-SHA512 (100 000 rounds).
fn password_hash(password: &str) -> String {
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);
    let mut key = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password.as_bytes(), &salt, 100_000, &mut key);
    let b = base64::engine::general_purpose::STANDARD;
    format!("@ByteArray({}:{})", b.encode(salt), b.encode(key))
}

/// A new port and random password are written at every start; the Web UI listens on 127.0.0.1 only.
fn write_config(profile: &Path, port: u16, password: &str, gui: bool) -> std::io::Result<()> {
    let dir = profile.join("qBittorrent").join("config");
    std::fs::create_dir_all(&dir)?;
    let mut lines = vec![
        "[LegalNotice]".to_string(),
        "Accepted=true".into(),
        "".into(),
        "[BitTorrent]".into(),
        "Session\\QueueingSystemEnabled=false".into(), // Compass runs the queue
        "Session\\TorrentContentLayout=Original".into(),
        "Session\\TempPathEnabled=false".into(),
        "Session\\AddExtensionToIncompleteFiles=false".into(),
        "Session\\GlobalMaxRatio=-1".into(),
        "".into(),
        "[Preferences]".into(),
        "WebUI\\Enabled=true".into(),
        "WebUI\\Address=127.0.0.1".into(),
        format!("WebUI\\Port={port}"),
        "WebUI\\UseUPnP=false".into(),
        "WebUI\\Username=torrentbul".into(),
        format!("WebUI\\Password_PBKDF2=\"{}\"", password_hash(password)),
        "WebUI\\LocalHostAuth=true".into(),
        "WebUI\\CSRFProtection=false".into(),
        "WebUI\\HostHeaderValidation=false".into(),
        "General\\UseRandomPort=false".into(),
    ];
    if gui {
        for l in [
            "General\\StartMinimized=true",
            "General\\SystemTrayEnabled=true",
            "General\\MinimizeToTray=true",
            "General\\CloseToTray=true",
            "Advanced\\confirmTorrentDeletion=false",
            "General\\ExitConfirm=false",
        ] {
            lines.push(l.into());
        }
    }
    lines.push(String::new());
    std::fs::write(dir.join("qBittorrent.conf"), lines.join("\n"))
}

// ---------------------------------------------------------------- Web API

#[derive(Clone)]
pub struct QbApi {
    base: String,
    username: String,
    password: String,
    client: reqwest::Client,
    logged_in: Arc<AtomicBool>,
}

pub enum Body {
    None,
    Form(Vec<(String, String)>),
    Multipart(Box<dyn Fn() -> reqwest::multipart::Form + Send + Sync>),
}

pub fn form(pairs: &[(&str, &str)]) -> Body {
    Body::Form(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
}

impl QbApi {
    pub fn new(base: &str, username: &str, password: &str) -> Self {
        let client = reqwest::Client::builder().cookie_store(true).no_proxy().timeout(Duration::from_secs(30)).build().expect("qb client");
        QbApi { base: base.into(), username: username.into(), password: password.into(), client, logged_in: Arc::new(AtomicBool::new(false)) }
    }

    async fn login(&self) -> Result<(), String> {
        let res = self
            .client
            .post(format!("{}api/v2/auth/login", self.base))
            .form(&[("username", &self.username), ("password", &self.password)])
            .send()
            .await
            .map_err(|e| format!("connect ECONNREFUSED qBittorrent ({e})"))?;
        let ok = res.status().is_success();
        let text = res.text().await.unwrap_or_default();
        if !ok || !text.to_lowercase().contains("ok") {
            return Err("qBittorrent girişi reddedildi.".into());
        }
        self.logged_in.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn call(&self, get: bool, path: &str, body: &Body) -> Result<reqwest::Response, String> {
        for attempt in 0..2 {
            if !self.logged_in.load(Ordering::SeqCst) {
                self.login().await?;
            }
            let url = format!("{}api/v2/{}", self.base, path);
            let req = if get { self.client.get(url) } else { self.client.post(url) };
            let req = match body {
                Body::None => req,
                Body::Form(pairs) => req.form(pairs),
                Body::Multipart(make) => req.multipart(make()),
            };
            let res = req.send().await.map_err(|e| format!("connect ECONNREFUSED qBittorrent {path} ({e})"))?;
            if res.status().as_u16() == 403 && attempt == 0 {
                self.logged_in.store(false, Ordering::SeqCst);
                continue;
            }
            if !res.status().is_success() {
                let status = res.status().as_u16();
                let text = res.text().await.unwrap_or_default();
                return Err(format!("qBittorrent {path}: {status} {text}").trim().to_string());
            }
            return Ok(res);
        }
        unreachable!()
    }

    fn with_query(path: &str, params: &[(&str, &str)]) -> String {
        if params.is_empty() {
            return path.into();
        }
        let q: String = url::form_urlencoded::Serializer::new(String::new()).extend_pairs(params).finish();
        format!("{path}?{q}")
    }

    pub async fn get_json(&self, path: &str, params: &[(&str, &str)]) -> Result<Value, String> {
        let res = self.call(true, &Self::with_query(path, params), &Body::None).await?;
        res.json().await.map_err(|e| e.to_string())
    }
    pub async fn get_text(&self, path: &str) -> Result<String, String> {
        self.call(true, path, &Body::None).await?.text().await.map_err(|e| e.to_string())
    }
    pub async fn get_bytes(&self, path: &str, params: &[(&str, &str)]) -> Result<Vec<u8>, String> {
        let res = self.call(true, &Self::with_query(path, params), &Body::None).await?;
        Ok(res.bytes().await.map_err(|e| e.to_string())?.to_vec())
    }
    pub async fn post(&self, path: &str, body: Body) -> Result<String, String> {
        self.call(false, path, &body).await?.text().await.map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------- process

pub struct Session {
    pub api: QbApi,
    child: Mutex<Option<std::process::Child>>,
}

impl Session {
    fn exited(&self) -> bool {
        let mut g = self.child.lock().unwrap();
        match g.as_mut() {
            None => true,
            Some(c) => !matches!(c.try_wait(), Ok(None)),
        }
    }

    pub async fn stop(&self) {
        if self.exited() {
            return;
        }
        let _ = self.api.post("app/shutdown", Body::None).await;
        for _ in 0..30 {
            if self.exited() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

const PID_FILE: &str = "torrent-bul.pid";

/// A process that has exited but not been collected (zombie) counts as gone.
#[cfg(unix)]
fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    if unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => !stat.rsplit(')').next().is_some_and(|rest| rest.trim_start().starts_with('Z')),
        Err(_) => !cfg!(target_os = "linux"),
    }
}

/// If the app crashes, the qBittorrent it started stays open and the same
/// profile cannot be opened twice. A recorded process still using this profile is stopped.
async fn stop_stale(profile: &Path) {
    let Ok(text) = std::fs::read_to_string(profile.join(PID_FILE)) else { return };
    let Ok(pid) = text.trim().parse::<i32>() else { return };
    if pid <= 0 {
        return;
    }
    #[cfg(unix)]
    {
        if cfg!(target_os = "linux") {
            match std::fs::read(format!("/proc/{pid}/cmdline")) {
                Ok(cmd) if String::from_utf8_lossy(&cmd).contains(&*profile.to_string_lossy()) => {}
                _ => return, // another process, or none
            }
        }
        // SAFETY: plain signal calls on a pid we recorded and checked above.
        unsafe {
            if libc::kill(pid, libc::SIGTERM) != 0 {
                return;
            }
        }
        for _ in 0..50 {
            if !alive(pid) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    }
}

pub async fn launch(profile: &Path, found: Option<(PathBuf, bool)>, timeout: Duration) -> Result<Session, String> {
    let (bin, gui) = found.ok_or(NOT_FOUND_MESSAGE)?;
    let port = free_port().map_err(|e| e.to_string())?;
    let mut raw = [0u8; 18];
    rand::thread_rng().fill_bytes(&mut raw);
    let password = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw);
    stop_stale(profile).await; // it writes its own settings on exit; ours must come after
    write_config(profile, port, &password, gui).map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new(&bin);
    cmd.arg(format!("--profile={}", profile.display()))
        .arg(format!("--webui-port={port}"))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let child = cmd.spawn().map_err(|e| format!("qBittorrent başlatılamadı ({e})."))?;
    let _ = std::fs::write(profile.join(PID_FILE), child.id().to_string());
    let session = Session { api: QbApi::new(&format!("http://127.0.0.1:{port}/"), "torrentbul", &password), child: Mutex::new(Some(child)) };
    let deadline = Instant::now() + timeout;
    loop {
        if session.exited() {
            let code = session.child.lock().unwrap().as_mut().and_then(|c| c.try_wait().ok().flatten()).map(|s| s.to_string());
            return Err(format!("qBittorrent başlatılamadı ({}).", code.unwrap_or_else(|| "signal".into())));
        }
        if session.api.get_text("app/version").await.is_ok() {
            return Ok(session);
        }
        if Instant::now() > deadline {
            if let Some(c) = session.child.lock().unwrap().as_mut() {
                let _ = c.kill();
            }
            return Err("qBittorrent zamanında açılmadı.".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// qBittorrent 5 uses stop/start instead of pause/resume.
async fn control(api: &QbApi, pause: bool, hashes: &str) -> Result<(), String> {
    let (old, new) = if pause { ("pause", "stop") } else { ("resume", "start") };
    match api.post(&format!("torrents/{old}"), form(&[("hashes", hashes)])).await {
        Ok(_) => Ok(()),
        Err(e) if e.contains(": 404") => api.post(&format!("torrents/{new}"), form(&[("hashes", hashes)])).await.map(|_| ()),
        Err(e) => Err(e),
    }
}

pub enum Source {
    Bytes(Vec<u8>),
    Magnet(String),
}

/// Torznab/Jackett download links often redirect to a magnet address, which an
/// HTTP client cannot follow. Redirects are followed by hand; if they end at a
/// magnet, that is used.
pub async fn fetch_torrent_source(url: &str, net: &Net) -> Result<Source, String> {
    let mut current = url.to_string();
    for _ in 0..6 {
        let res = net
            .no_redirect
            .get(&current)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| format!("Error downloading torrent: {}", root_cause(&e)))?;
        let status = res.status().as_u16();
        let location = res.headers().get("location").and_then(|v| v.to_str().ok()).map(String::from);
        if (300..400).contains(&status) {
            if let Some(loc) = location {
                if loc.to_lowercase().starts_with("magnet:") {
                    return Ok(Source::Magnet(loc));
                }
                current = url::Url::parse(&current).and_then(|b| b.join(&loc)).map(|u| u.to_string()).map_err(|e| e.to_string())?;
                continue;
            }
        }
        // 4xx is permanent (expired link, wrong key); retrying is pointless.
        if (400..500).contains(&status) {
            return Err(format!(
                "Torrent bağlantısı açılamadı (HTTP {status}); bağlantının süresi dolmuş ya da API anahtarı yanlış olabilir."
            ));
        }
        if !res.status().is_success() {
            return Err(format!("Error downloading torrent: HTTP {status}"));
        }
        return Ok(Source::Bytes(res.bytes().await.map_err(|e| format!("Error downloading torrent: {e}"))?.to_vec()));
    }
    Err("Torrent bağlantısı çok fazla yönlendirme yaptı.".into())
}

fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    let s = cur.to_string();
    if e.to_string().contains("timed out") || s.contains("timed out") {
        "ETIMEDOUT".into()
    } else if s.contains("refused") {
        "ECONNREFUSED".into()
    } else if s.contains("dns") || s.contains("resolve") || s.contains("lookup") {
        "ENOTFOUND".into()
    } else {
        s
    }
}

// ---------------------------------------------------------------- torrents

#[derive(Clone, Debug, Default)]
pub struct LiveFile {
    pub index: usize,
    /// Path inside the save folder, as qBittorrent reports it.
    pub path: String,
    pub name: String,
    pub length: u64,
    pub offset: u64,
    pub progress: f64,
    pub priority: i64,
    pub downloaded: u64,
    reported: bool,
}

impl LiveFile {
    pub fn done(&self) -> bool {
        self.progress >= 1.0
    }
    fn update(&mut self, f: &Value) {
        self.progress = f["progress"].as_f64().unwrap_or(0.0);
        self.priority = f["priority"].as_i64().unwrap_or(0);
        self.downloaded = (self.length as f64 * self.progress).round() as u64;
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub info_hash: Option<String>,
    pub name: Option<String>,
    pub files: Vec<LiveFile>,
    pub piece_size: u64,
    pub length: u64,
    pub progress: f64,
    pub downloaded: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub uploaded: u64,
    pub peers: u64,
    pub time_remaining: Option<i64>,
    pub torrent_file: Option<Vec<u8>>,
    pub done: bool,
    pub destroyed: bool,
    pub has_metadata: bool,
    added: bool,
    forget: bool,
    sequential: bool,
    have_pieces: Vec<u8>,
}

#[derive(Debug, Clone)]
pub enum Event {
    Metadata,
    FileDone(usize),
    Done,
    Error(String),
}

type Op = Box<dyn FnOnce(QbApi) -> BoxFuture<'static, Result<(), String>> + Send>;

pub struct Torrent {
    pub id: u64,
    pub save_path: String,
    deselect: bool,
    st: Mutex<State>,
    ops: mpsc::UnboundedSender<Op>,
}

impl Torrent {
    pub fn state(&self) -> State {
        self.st.lock().unwrap().clone()
    }
    pub fn destroyed(&self) -> bool {
        self.st.lock().unwrap().destroyed
    }
    fn info_hash(&self) -> String {
        self.st.lock().unwrap().info_hash.clone().unwrap_or_default()
    }
}

type Starter = Box<dyn Fn() -> BoxFuture<'static, Result<Session, String>> + Send + Sync>;

struct Inner {
    start: Starter,
    session: tokio::sync::Mutex<Option<Arc<Session>>>,
    current: Mutex<Option<Arc<Session>>>,
    limits: Mutex<(u64, u64)>,
    live: Mutex<Vec<Arc<Torrent>>>,
    events: mpsc::UnboundedSender<(u64, Event)>,
    closing: AtomicBool,
    next_id: AtomicU64,
    poll: Duration,
    net: Net,
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// `profile`: qBittorrent's own profile folder inside the app's data folder.
    pub fn new(profile: PathBuf, net: Net) -> (Self, mpsc::UnboundedReceiver<(u64, Event)>) {
        let start: Starter = Box::new(move || {
            let profile = profile.clone();
            Box::pin(async move { launch(&profile, find_qbittorrent(), Duration::from_secs(20)).await })
        });
        Self::with_starter(start, Duration::from_secs(1), net)
    }

    pub fn with_starter(start: Starter, poll: Duration, net: Net) -> (Self, mpsc::UnboundedReceiver<(u64, Event)>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            start,
            session: tokio::sync::Mutex::new(None),
            current: Mutex::new(None),
            limits: Mutex::new((0, 0)),
            live: Mutex::new(vec![]),
            events: tx,
            closing: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            poll,
            net,
        });
        let engine = Engine { inner };
        let e = engine.clone();
        tokio::spawn(async move { e.poll_loop().await });
        (engine, rx)
    }

    async fn ensure(&self) -> Result<QbApi, String> {
        if self.inner.closing.load(Ordering::SeqCst) {
            return Err("Uygulama kapanıyor.".into());
        }
        let mut guard = self.inner.session.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.api.clone());
        }
        let s = Arc::new((self.inner.start)().await?);
        let _ = s.api.post("torrents/createCategory", form(&[("category", CATEGORY), ("savePath", "")])).await;
        // Leftovers from the previous session are stopped; Compass decides which ones go on.
        if let Ok(hashes) = self.category_hashes(&s.api).await {
            let _ = control(&s.api, true, &hashes).await;
        }
        let _ = self.apply_limits(&s.api).await;
        *guard = Some(s.clone());
        *self.inner.current.lock().unwrap() = Some(s.clone());
        Ok(s.api.clone())
    }

    fn current(&self) -> Option<QbApi> {
        self.inner.current.lock().unwrap().as_ref().map(|s| s.api.clone())
    }

    async fn category_hashes(&self, api: &QbApi) -> Result<String, String> {
        let list = api.get_json("torrents/info", &[("category", CATEGORY)]).await?;
        let hashes: Vec<&str> = list.as_array().map(|a| a.iter().filter_map(|t| t["hash"].as_str()).collect()).unwrap_or_default();
        Ok(if hashes.is_empty() { "none".into() } else { hashes.join("|") })
    }

    async fn apply_limits(&self, api: &QbApi) -> Result<(), String> {
        let (down, up) = *self.inner.limits.lock().unwrap();
        api.post("transfer/setDownloadLimit", form(&[("limit", &down.to_string())])).await?;
        api.post("transfer/setUploadLimit", form(&[("limit", &up.to_string())])).await?;
        Ok(())
    }

    fn emit(&self, t: &Torrent, ev: Event) {
        let _ = self.inner.events.send((t.id, ev));
    }

    /// Stops a torrent after an error and tells the manager.
    pub fn fail(&self, t: &Arc<Torrent>, err: String) {
        let (hash, added) = {
            let mut s = t.st.lock().unwrap();
            if s.destroyed {
                return;
            }
            s.destroyed = true;
            (s.info_hash.clone(), s.added)
        };
        self.inner.live.lock().unwrap().retain(|x| x.id != t.id);
        if let (Some(h), true, Some(api)) = (hash, added, self.current()) {
            tokio::spawn(async move {
                let _ = control(&api, true, &h).await;
            });
        }
        self.emit(t, Event::Error(err));
    }

    /// Requests for one torrent run in order (e.g. file selection before start).
    fn queue(&self, t: &Arc<Torrent>, op: Op) {
        let _ = t.ops.send(op);
    }

    async fn release_after_add(api: &QbApi, hash: &str, forget: bool) {
        if forget {
            let _ = api.post("torrents/delete", form(&[("hashes", hash), ("deleteFiles", "false")])).await;
        } else {
            let _ = control(api, true, hash).await;
        }
    }

    /// A torrent leaving the engine: with `forget` it is removed from
    /// qBittorrent's list (files stay); otherwise it is stopped and kept waiting.
    fn release(&self, t: &Arc<Torrent>, forget: bool) {
        let (hash, added) = {
            let mut s = t.st.lock().unwrap();
            if s.destroyed {
                return;
            }
            s.destroyed = true;
            s.forget = forget;
            (s.info_hash.clone(), s.added)
        };
        self.inner.live.lock().unwrap().retain(|x| x.id != t.id);
        // While still being added, add() itself applies the decision.
        if let (Some(h), true, Some(api)) = (hash, added, self.current()) {
            tokio::spawn(async move { Self::release_after_add(&api, &h, forget).await });
        }
    }

    pub fn add(&self, source: Source, save_path: &str, deselect: bool) -> Arc<Torrent> {
        let (tx, mut rx) = mpsc::unbounded_channel::<Op>();
        let t = Arc::new(Torrent {
            id: self.inner.next_id.fetch_add(1, Ordering::SeqCst),
            save_path: save_path.to_string(),
            deselect,
            st: Mutex::new(State::default()),
            ops: tx,
        });
        self.inner.live.lock().unwrap().push(t.clone());
        // Worker that runs this torrent's requests one by one.
        let engine = self.clone();
        let tw = t.clone();
        tokio::spawn(async move {
            while let Some(op) = rx.recv().await {
                if tw.destroyed() {
                    continue;
                }
                let res = match engine.ensure().await {
                    Ok(api) => op(api).await,
                    Err(e) => Err(e),
                };
                if let Err(e) = res {
                    engine.fail(&tw, e);
                }
            }
        });
        let engine = self.clone();
        let t2 = t.clone();
        self.queue(
            &t,
            Box::new(move |api| {
                Box::pin(async move {
                    let (buf, magnet) = match source {
                        Source::Bytes(b) => (Some(b), None),
                        Source::Magnet(m) if m.to_lowercase().starts_with("magnet:") => (None, Some(m)),
                        Source::Magnet(url) => match fetch_torrent_source(&url, &engine.inner.net).await? {
                            Source::Bytes(b) => (Some(b), None),
                            Source::Magnet(m) => (None, Some(m)),
                        },
                    };
                    let hash = match (&buf, &magnet) {
                        (Some(b), _) => parse_torrent(b).map_err(|_| "Invalid torrent identifier".to_string())?.info_hash,
                        (_, Some(m)) => magnet_info_hash(m).ok_or("Invalid torrent identifier")?,
                        _ => unreachable!(),
                    };
                    {
                        let mut s = t2.st.lock().unwrap();
                        s.info_hash = Some(hash.clone());
                        if buf.is_some() {
                            s.torrent_file = buf.clone();
                        }
                        if s.destroyed {
                            return Ok(());
                        }
                    }
                    // A torrent that was paused and kept is not added again: it goes on without re-checking files.
                    let existing = api.get_json("torrents/info", &[("hashes", &hash)]).await?;
                    if existing.as_array().is_some_and(|a| !a.is_empty()) {
                        control(&api, true, &hash).await?; // no downloading until the selection is made again
                    } else {
                        std::fs::create_dir_all(&t2.save_path).map_err(|e| e.to_string())?;
                        let save = t2.save_path.clone();
                        let body = match buf {
                            Some(b) => Body::Multipart(Box::new(move || {
                                reqwest::multipart::Form::new()
                                    .text("savepath", save.clone())
                                    .text("category", CATEGORY)
                                    .text("contentLayout", "Original")
                                    .part("torrents", reqwest::multipart::Part::bytes(b.clone()).file_name("item.torrent"))
                                    .text("paused", "true")
                                    .text("stopped", "true") // qBittorrent 5
                            })),
                            None => {
                                let m = magnet.unwrap();
                                Body::Multipart(Box::new(move || {
                                    reqwest::multipart::Form::new()
                                        .text("savepath", save.clone())
                                        .text("category", CATEGORY)
                                        .text("contentLayout", "Original")
                                        .text("urls", m.clone())
                                        .text("stopCondition", "MetadataReceived")
                                }))
                            }
                        };
                        let reply = api.post("torrents/add", body).await?;
                        if !reply.to_lowercase().contains("ok") {
                            return Err("qBittorrent torrenti eklemedi.".into());
                        }
                    }
                    let (destroyed, forget) = {
                        let mut s = t2.st.lock().unwrap();
                        s.added = true;
                        (s.destroyed, s.forget)
                    };
                    if destroyed {
                        Self::release_after_add(&api, &hash, forget).await;
                    }
                    Ok(())
                })
            }),
        );
        t
    }

    /// Pausing: the torrent stops in qBittorrent; resuming is adding it again.
    pub fn pause(&self, t: &Arc<Torrent>) {
        self.release(t, false)
    }

    /// Let go entirely: removed from qBittorrent's list, files stay on disk.
    pub fn remove(&self, t: &Arc<Torrent>) {
        self.release(t, true)
    }

    async fn set_priority(api: &QbApi, t: &Torrent, indexes: &[usize], priority: i64) -> Result<(), String> {
        if indexes.is_empty() {
            return Ok(());
        }
        let hash = {
            let mut s = t.st.lock().unwrap();
            for &i in indexes {
                if let Some(f) = s.files.get_mut(i) {
                    f.priority = priority;
                }
            }
            s.info_hash.clone().unwrap_or_default()
        };
        let ids = indexes.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("|");
        api.post("torrents/filePrio", form(&[("hash", &hash), ("id", &ids), ("priority", &priority.to_string())])).await.map(|_| ())
    }

    fn queue_priority(&self, t: &Arc<Torrent>, which: Option<Vec<usize>>, priority: i64) {
        let t2 = t.clone();
        self.queue(
            t,
            Box::new(move |api| {
                Box::pin(async move {
                    let idx = which.unwrap_or_else(|| (0..t2.st.lock().unwrap().files.len()).collect());
                    Self::set_priority(&api, &t2, &idx, priority).await
                })
            }),
        );
    }

    pub fn select_files(&self, t: &Arc<Torrent>, indexes: Vec<usize>) {
        self.queue_priority(t, Some(indexes), 1)
    }
    pub fn deselect_all(&self, t: &Arc<Torrent>) {
        self.queue_priority(t, None, 0)
    }
    pub fn select_all(&self, t: &Arc<Torrent>) {
        self.queue_priority(t, None, 1)
    }

    /// After the manager has made its choices on the file list: start downloading.
    /// Subtitles are small and the player asks for them at once, so selected ones go first.
    pub fn begin(&self, t: &Arc<Torrent>) {
        let t2 = t.clone();
        self.queue(
            t,
            Box::new(move |api| {
                Box::pin(async move {
                    let subs: Vec<usize> = t2
                        .state()
                        .files
                        .iter()
                        .filter(|f| f.priority > 0 && f.length <= 1024 * 1024 && is_subtitle(&f.name))
                        .map(|f| f.index)
                        .collect();
                    Self::set_priority(&api, &t2, &subs, 7).await?;
                    control(&api, false, &t2.info_hash()).await
                })
            }),
        );
    }

    /// Bytes per second; 0 = unlimited.
    pub fn throttle(&self, down: u64, up: u64) {
        *self.inner.limits.lock().unwrap() = (down, up);
        if let Some(api) = self.current() {
            let e = self.clone();
            tokio::spawn(async move {
                let _ = e.apply_limits(&api).await;
            });
        }
    }

    // ------------------------------------------------------------ polling

    async fn poll_loop(self) {
        let mut failures = 0;
        loop {
            let fast = self.inner.live.lock().unwrap().iter().any(|t| !t.st.lock().unwrap().has_metadata);
            // Look often until the file list arrives.
            tokio::time::sleep(if fast { self.inner.poll.min(Duration::from_millis(250)) } else { self.inner.poll }).await;
            if self.inner.closing.load(Ordering::SeqCst) {
                return;
            }
            match self.poll().await {
                Ok(()) => failures = 0,
                Err(e) => {
                    // If qBittorrent closed, live torrents get a temporary error; the retry starts it again.
                    failures += 1;
                    if failures >= 3 {
                        failures = 0;
                        *self.inner.session.lock().await = None;
                        *self.inner.current.lock().unwrap() = None;
                        let live: Vec<_> = self.inner.live.lock().unwrap().clone();
                        for t in live {
                            self.fail(&t, format!("connect ECONNREFUSED qBittorrent ({e})"));
                        }
                    }
                }
            }
        }
    }

    async fn poll(&self) -> Result<(), String> {
        let tracked: Vec<Arc<Torrent>> =
            self.inner.live.lock().unwrap().iter().filter(|t| { let s = t.st.lock().unwrap(); s.added && s.info_hash.is_some() }).cloned().collect();
        if tracked.is_empty() {
            return Ok(());
        }
        let Some(api) = self.current() else { return Ok(()) };
        let list = api.get_json("torrents/info", &[("category", CATEGORY)]).await?;
        let infos: HashMap<String, Value> =
            list.as_array().cloned().unwrap_or_default().into_iter().filter_map(|i| Some((i["hash"].as_str()?.to_string(), i))).collect();
        for t in tracked {
            if t.destroyed() {
                continue;
            }
            let hash = t.info_hash();
            let Some(info) = infos.get(&hash) else {
                self.fail(&t, "Torrent qBittorrent listesinden kaldırıldı.".into());
                continue;
            };
            let state = info["state"].as_str().unwrap_or_default().to_string();
            if state == "error" {
                self.fail(&t, "qBittorrent bu torrentte hata bildirdi (klasöre yazılamıyor ya da disk dolu olabilir).".into());
                continue;
            }
            if state == "missingFiles" {
                self.fail(&t, "İndirilen dosyalar bulunamadı; taşınmış ya da silinmiş olabilir.".into());
                continue;
            }
            let total = info["total_size"].as_u64().unwrap_or(0);
            let has_metadata = {
                let mut s = t.st.lock().unwrap();
                s.name = info["name"].as_str().map(String::from);
                s.download_speed = info["dlspeed"].as_u64().unwrap_or(0);
                s.upload_speed = info["upspeed"].as_u64().unwrap_or(0);
                s.uploaded = info["uploaded_session"].as_u64().unwrap_or(0);
                s.peers = info["num_seeds"].as_u64().unwrap_or(0) + info["num_leechs"].as_u64().unwrap_or(0);
                let eta = info["eta"].as_i64().unwrap_or(NO_ETA);
                s.time_remaining = (eta < NO_ETA).then_some(eta * 1000);
                s.has_metadata
            };
            if !has_metadata {
                if state == "metaDL" || total == 0 {
                    continue;
                }
                if let Err(e) = self.load_metadata(&api, &t, info).await {
                    if !t.destroyed() {
                        return Err(e);
                    }
                }
                continue;
            }
            let files = api.get_json("torrents/files", &[("hash", &hash)]).await?;
            let mut events = vec![];
            {
                let mut s = t.st.lock().unwrap();
                for f in files.as_array().cloned().unwrap_or_default() {
                    let i = f["index"].as_u64().unwrap_or(0) as usize;
                    if let Some(lf) = s.files.get_mut(i) {
                        lf.update(&f);
                    }
                }
                s.length = total;
                s.downloaded = info["completed"].as_u64().unwrap_or(0);
                s.progress = info["progress"].as_f64().unwrap_or(0.0);
                if BUSY.contains(&state.as_str()) {
                    continue;
                }
                for f in s.files.iter_mut() {
                    if f.priority > 0 && f.done() && !f.reported {
                        f.reported = true;
                        events.push(Event::FileDone(f.index));
                    }
                }
                let wanted: Vec<&LiveFile> = s.files.iter().filter(|f| f.priority > 0).collect();
                if !s.done && !wanted.is_empty() && wanted.iter().all(|f| f.done()) {
                    s.done = true;
                    events.push(Event::Done);
                }
            }
            for ev in events {
                self.emit(&t, ev);
            }
        }
        Ok(())
    }

    async fn load_metadata(&self, api: &QbApi, t: &Arc<Torrent>, info: &Value) -> Result<(), String> {
        let hash = t.info_hash();
        let q = [("hash", hash.as_str())];
        let (files, props) = tokio::join!(api.get_json("torrents/files", &q), api.get_json("torrents/properties", &q));
        let (mut files, props) = (files?.as_array().cloned().unwrap_or_default(), props?);
        if t.destroyed() {
            return Ok(());
        }
        files.sort_by_key(|f| f["index"].as_u64().unwrap_or(0));
        let total = info["total_size"].as_u64().unwrap_or(0);
        let piece = props["piece_size"].as_u64().unwrap_or(0);
        let need_export = {
            let mut s = t.st.lock().unwrap();
            let mut offset = 0;
            s.files = files
                .iter()
                .map(|f| {
                    let path = f["name"].as_str().unwrap_or_default().to_string();
                    let length = f["size"].as_u64().unwrap_or(0);
                    let mut lf = LiveFile {
                        index: f["index"].as_u64().unwrap_or(0) as usize,
                        name: path.rsplit(['/', '\\']).next().unwrap_or_default().to_string(),
                        path,
                        length,
                        offset,
                        ..Default::default()
                    };
                    lf.update(f);
                    offset += length;
                    lf
                })
                .collect();
            s.piece_size = piece;
            s.length = total;
            s.torrent_file.is_none()
        };
        // When a .torrent is at hand it is kept: the file qBittorrent 5 exports has no web
        // seeds (url-list), and an Internet Archive torrent added again from it never
        // downloads. Exporting is only for magnets.
        if need_export {
            if let Ok(b) = api.get_bytes("torrents/export", &[("hash", &hash)]).await {
                t.st.lock().unwrap().torrent_file = Some(b);
            }
        }
        // A kept torrent may carry its old selection; the default is set explicitly every time.
        let want = if t.deselect { 0 } else { 1 };
        let change: Vec<usize> = t.state().files.iter().filter(|f| f.priority != want).map(|f| f.index).collect();
        Self::set_priority(api, t, &change, want).await?;
        if t.destroyed() {
            return Ok(());
        }
        t.st.lock().unwrap().has_metadata = true;
        self.emit(t, Event::Metadata);
        Ok(())
    }

    // ------------------------------------------------------------ streaming

    /// Watching while downloading: the file being read gets top priority, the
    /// torrent downloads in order and first/last pieces come first (players read the end too).
    async fn streaming(&self, t: &Arc<Torrent>, index: usize) -> Result<(), String> {
        let api = self.ensure().await?;
        let f = t.state().files.get(index).cloned().ok_or("Dosya yok.")?;
        if f.priority > 0 && f.priority < 7 && !f.done() {
            Self::set_priority(&api, t, &[index], 7).await?;
        }
        {
            let mut s = t.st.lock().unwrap();
            if s.sequential {
                return Ok(());
            }
            s.sequential = true;
        }
        let hash = t.info_hash();
        let info = api.get_json("torrents/info", &[("hashes", &hash)]).await?;
        if let Some(i) = info.as_array().and_then(|a| a.first()) {
            if i["seq_dl"].as_bool() != Some(true) {
                api.post("torrents/toggleSequentialDownload", form(&[("hashes", &hash)])).await?;
            }
            if i["f_l_piece_prio"].as_bool() != Some(true) {
                api.post("torrents/toggleFirstLastPiecePrio", form(&[("hashes", &hash)])).await?;
            }
        }
        Ok(())
    }

    async fn wait_piece(&self, t: &Arc<Torrent>, piece: usize) -> Result<(), String> {
        let api = self.ensure().await?;
        loop {
            if t.destroyed() {
                return Err("İndirme durduruldu.".into());
            }
            let have = t.st.lock().unwrap().have_pieces.get(piece).copied();
            if have == Some(2) {
                return Ok(());
            }
            let states = api.get_json("torrents/pieceStates", &[("hash", &t.info_hash())]).await?;
            let v: Vec<u8> = states.as_array().map(|a| a.iter().map(|x| x.as_u64().unwrap_or(0) as u8).collect()).unwrap_or_default();
            let ok = v.get(piece) == Some(&2);
            t.st.lock().unwrap().have_pieces = v;
            if ok {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    /// Reads part of a file while it downloads: waits until each needed piece is there.
    pub fn read_stream(&self, t: &Arc<Torrent>, index: usize, start: u64, end: u64) -> mpsc::Receiver<Result<bytes::Bytes, std::io::Error>> {
        let (tx, rx) = mpsc::channel(4);
        let engine = self.clone();
        let t = t.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncSeekExt};
            let fail = |e: String| std::io::Error::other(e);
            if let Err(e) = engine.streaming(&t, index).await {
                let _ = tx.send(Err(fail(e))).await;
                return;
            }
            let st = t.state();
            let Some(file) = st.files.get(index).cloned() else { return };
            let full = Path::new(&t.save_path).join(&file.path);
            let mut fh = loop {
                match tokio::fs::File::open(&full).await {
                    Ok(f) => break f,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound && !t.destroyed() => tokio::time::sleep(Duration::from_millis(300)).await,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                }
            };
            let piece_size = st.piece_size.max(1);
            let mut pos = start;
            while pos <= end {
                let piece = ((file.offset + pos) / piece_size) as usize;
                if let Err(e) = engine.wait_piece(&t, piece).await {
                    let _ = tx.send(Err(fail(e))).await;
                    return;
                }
                let piece_end = (piece as u64 + 1) * piece_size - file.offset - 1;
                let upto = end.min(piece_end);
                let mut buf = vec![0u8; (upto - pos + 1) as usize];
                if let Err(e) = fh.seek(std::io::SeekFrom::Start(pos)).await {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
                let n = match fh.read(&mut buf).await {
                    Ok(0) => {
                        let _ = tx.send(Err(fail("Dosya okunamadı.".into()))).await;
                        return;
                    }
                    Ok(n) => n,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                };
                buf.truncate(n);
                if tx.send(Ok(bytes::Bytes::from(buf))).await.is_err() {
                    return; // the player went away
                }
                pos += n as u64;
            }
        });
        rx
    }

    pub async fn destroy(&self) {
        self.inner.closing.store(true, Ordering::SeqCst);
        for t in self.inner.live.lock().unwrap().drain(..) {
            t.st.lock().unwrap().destroyed = true;
        }
        let s = self.inner.session.lock().await.take();
        *self.inner.current.lock().unwrap() = None;
        if let Some(s) = s {
            s.stop().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::testserver;
    use axum::response::Response;

    #[tokio::test]
    async fn jackett_redirect_to_magnet_uses_the_magnet_and_4xx_is_permanent() {
        let magnet = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=Film";
        let server = testserver::start(move |req| {
            let r = Response::builder();
            match req.uri().path() {
                "/dl/magnet" => r.status(302).header("location", magnet).body(axum::body::Body::empty()).unwrap(),
                "/dl/zincir" => r.status(301).header("location", "/dl/dosya").body(axum::body::Body::empty()).unwrap(),
                "/dl/dosya" => r.body(axum::body::Body::from("d4:infod4:name1:xee")).unwrap(),
                _ => r.status(404).body(axum::body::Body::empty()).unwrap(),
            }
        })
        .await;
        let net = Net::new();
        match fetch_torrent_source(&format!("{}/dl/magnet", server.base), &net).await.unwrap() {
            Source::Magnet(m) => assert_eq!(m, magnet),
            _ => panic!("expected magnet"),
        }
        match fetch_torrent_source(&format!("{}/dl/zincir", server.base), &net).await.unwrap() {
            Source::Bytes(b) => assert_eq!(b, b"d4:infod4:name1:xee"),
            _ => panic!("expected bytes"),
        }
        let err = fetch_torrent_source(&format!("{}/dl/yok", server.base), &net).await.err().unwrap();
        assert!(err.contains("HTTP 404"));
        assert!(!crate::downloads::is_transient_error(&err), "4xx is not retried");
        let err = fetch_torrent_source("http://127.0.0.1:9/x", &net).await.err().unwrap();
        assert!(crate::downloads::is_transient_error(&err), "no connection is retried: {err}");
    }

    #[tokio::test]
    async fn stale_qbittorrent_from_a_crashed_session_is_stopped_and_a_new_one_opens() {
        let Some(found) = find_qbittorrent() else {
            eprintln!("qbittorrent-nox kurulu değil; atlandı");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let first = launch(dir.path(), Some(found.clone()), Duration::from_secs(20)).await.unwrap();
        let old: i32 = std::fs::read_to_string(dir.path().join(PID_FILE)).unwrap().trim().parse().unwrap();
        std::mem::forget(first); // no stop: as if the app crashed
        let qb = launch(dir.path(), Some(found), Duration::from_secs(20)).await.unwrap();
        assert!(qb.api.get_text("app/version").await.unwrap().starts_with('v'));
        #[cfg(unix)]
        assert!(!alive(old), "old process is gone");
        qb.stop().await;
    }
}
