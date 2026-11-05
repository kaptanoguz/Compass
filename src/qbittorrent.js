// qBittorrent indirme motoru. Uygulama qBittorrent'i kendi ayrı profiliyle
// başlatır (kullanıcının kendi qBittorrent ayarlarına ve listesine dokunmaz),
// Web API'si üzerinden konuşur ve her torrenti downloads.js'in beklediği
// biçimde (metadata/ready/done/error olayları, files, progress...) sunar.
import fs from 'node:fs';
import os from 'node:os';
import net from 'node:net';
import path from 'node:path';
import crypto from 'node:crypto';
import { spawn } from 'node:child_process';
import { EventEmitter } from 'node:events';
import { Readable } from 'node:stream';
import parseTorrent from 'parse-torrent';

const CATEGORY = 'torrent-bul'; // eski adla kalır: önceki indirmeler aynı kategoride bulunur
const NO_ETA = 8640000; // qBittorrent "sonsuz" süre değeri
const BUSY = new Set(['metaDL', 'checkingDL', 'checkingUP', 'checkingResumeData', 'allocating', 'moving']);
const SUBTITLE = /\.(srt|vtt|ass|ssa|sub)$/i;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export const NOT_FOUND_MESSAGE = 'qBittorrent bulunamadı. Terminalde "sudo apt install qbittorrent-nox" ile kurup uygulamayı yeniden aç.';

// Pencere açmayan qbittorrent-nox önce denenir; yoksa masaüstü sürümü (tepsiye küçülmüş).
export function findQbittorrent(env = process.env) {
  if (env.QBITTORRENT_BIN) return { bin: env.QBITTORRENT_BIN, gui: !/nox/.test(env.QBITTORRENT_BIN) };
  const home = os.homedir();
  const candidates = process.platform === 'win32'
    ? [['C:\\Program Files\\qBittorrent\\qbittorrent.exe', true], ['C:\\Program Files (x86)\\qBittorrent\\qbittorrent.exe', true]]
    : process.platform === 'darwin'
      ? [['/Applications/qBittorrent.app/Contents/MacOS/qbittorrent', true]]
      : [
          ...(env.PATH || '').split(path.delimiter).filter(Boolean).flatMap((d) => [[path.join(d, 'qbittorrent-nox'), false]]),
          ...(env.PATH || '').split(path.delimiter).filter(Boolean).flatMap((d) => [[path.join(d, 'qbittorrent'), true]]),
          ['/var/lib/flatpak/exports/bin/org.qbittorrent.qBittorrent', true],
          [path.join(home, '.local/share/flatpak/exports/bin/org.qbittorrent.qBittorrent'), true],
          ['/snap/bin/qbittorrent-arnatious', true],
        ];
  for (const [bin, gui] of candidates) {
    try { fs.accessSync(bin, fs.constants.X_OK); return { bin, gui }; } catch { /* sıradaki */ }
  }
  return null;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => { const { port } = srv.address(); srv.close(() => resolve(port)); });
  });
}

// qBittorrent Web arayüzü şifresi PBKDF2-SHA512 (100 000 tur) olarak saklanır.
function passwordHash(password) {
  const salt = crypto.randomBytes(16);
  const key = crypto.pbkdf2Sync(password, salt, 100000, 64, 'sha512');
  return `@ByteArray(${salt.toString('base64')}:${key.toString('base64')})`;
}

// Her açılışta yeni bir port ve rastgele şifre yazılır; Web arayüzü yalnızca 127.0.0.1'i dinler.
function writeConfig(profileDir, { port, password, gui }) {
  const dir = path.join(profileDir, 'qBittorrent', 'config');
  fs.mkdirSync(dir, { recursive: true });
  const lines = [
    '[LegalNotice]', 'Accepted=true', '',
    '[BitTorrent]',
    'Session\\QueueingSystemEnabled=false', // sırayı Compass yönetir
    'Session\\TorrentContentLayout=Original',
    'Session\\TempPathEnabled=false',
    'Session\\AddExtensionToIncompleteFiles=false',
    'Session\\GlobalMaxRatio=-1',
    '',
    '[Preferences]',
    'WebUI\\Enabled=true',
    'WebUI\\Address=127.0.0.1',
    `WebUI\\Port=${port}`,
    'WebUI\\UseUPnP=false',
    'WebUI\\Username=torrentbul',
    `WebUI\\Password_PBKDF2="${passwordHash(password)}"`,
    'WebUI\\LocalHostAuth=true',
    'WebUI\\CSRFProtection=false',
    'WebUI\\HostHeaderValidation=false',
    'General\\UseRandomPort=false',
    ...(gui ? ['General\\StartMinimized=true', 'General\\SystemTrayEnabled=true', 'General\\MinimizeToTray=true', 'General\\CloseToTray=true', 'Advanced\\confirmTorrentDeletion=false', 'General\\ExitConfirm=false'] : []),
    '',
  ];
  fs.writeFileSync(path.join(dir, 'qBittorrent.conf'), lines.join('\n'));
}

export async function launchQbittorrent({ profileDir, found = findQbittorrent(), timeoutMs = 20000 }) {
  if (!found) throw new Error(NOT_FOUND_MESSAGE);
  const port = await freePort();
  const password = crypto.randomBytes(18).toString('base64url');
  await stopStale(profileDir); // kapanırken kendi ayarlarını yazar; yenisi ondan sonra yazılmalı
  writeConfig(profileDir, { port, password, gui: found.gui });
  const child = spawn(found.bin, [`--profile=${profileDir}`, `--webui-port=${port}`], { stdio: 'ignore', detached: false });
  if (child.pid) fs.writeFileSync(path.join(profileDir, 'torrent-bul.pid'), String(child.pid));
  let exited = null;
  child.on('exit', (code) => { exited = code ?? 'signal'; });
  child.on('error', (err) => { exited = err.message; });
  const url = `http://127.0.0.1:${port}/`;
  const api = createQbApi({ url, username: 'torrentbul', password });
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    if (exited !== null) throw new Error(`qBittorrent başlatılamadı (${exited}).`);
    try { await api.get('app/version'); break; } catch { /* henüz hazır değil */ }
    if (Date.now() > deadline) { child.kill(); throw new Error('qBittorrent zamanında açılmadı.'); }
    await sleep(200);
  }
  return {
    api,
    async stop() {
      if (exited !== null) return;
      try { await api.post('app/shutdown'); } catch { /* zaten kapanıyor */ }
      for (let i = 0; i < 30 && exited === null; i++) await sleep(100);
      if (exited === null) child.kill();
    },
  };
}

// Uygulama çökerse başlattığı qBittorrent açık kalır ve aynı profil ikinci kez açılamaz.
// Kayıtlı süreç hâlâ bu profille çalışıyorsa kapatılır.
async function stopStale(profileDir) {
  let pid;
  try { pid = Number(fs.readFileSync(path.join(profileDir, 'torrent-bul.pid'), 'utf8')); } catch { return; }
  if (!pid) return;
  if (process.platform === 'linux') {
    try {
      if (!fs.readFileSync(`/proc/${pid}/cmdline`, 'utf8').includes(profileDir)) return; // başka bir süreç
    } catch { return; } // süreç yok
  }
  try { process.kill(pid); } catch { return; }
  for (let i = 0; i < 50; i++) {
    try { process.kill(pid, 0); } catch { return; }
    await sleep(100);
  }
  try { process.kill(pid, 'SIGKILL'); } catch { /* kapandı */ }
}

export function createQbApi({ url, username, password, fetch: fetchImpl = fetch }) {
  let cookie = '';
  async function login() {
    const res = await fetchImpl(new URL('api/v2/auth/login', url), { method: 'POST', body: new URLSearchParams({ username, password }) });
    const text = await res.text();
    if (!res.ok || !/ok/i.test(text)) throw new Error('qBittorrent girişi reddedildi.');
    cookie = (res.headers.get('set-cookie') || '').split(';')[0];
  }
  async function call(method, p, body, raw = false, retry = true) {
    if (!cookie) await login();
    const res = await fetchImpl(new URL(`api/v2/${p}`, url), { method, body, headers: { cookie } });
    if (res.status === 403 && retry) { cookie = ''; return call(method, p, body, raw, false); }
    if (!res.ok) throw new Error(`qBittorrent ${p}: ${res.status} ${await res.text().catch(() => '')}`.trim());
    if (raw) return Buffer.from(await res.arrayBuffer());
    const type = res.headers.get('content-type') || '';
    if (type.includes('json')) return res.json();
    if (type.includes('text')) return res.text();
    return Buffer.from(await res.arrayBuffer());
  }
  return {
    get: (p, params, { raw = false } = {}) => call('GET', params ? `${p}?${new URLSearchParams(params)}` : p, undefined, raw),
    post: (p, params) => call('POST', p, params instanceof FormData ? params : new URLSearchParams(params || {})),
  };
}

class QbFile extends EventEmitter {
  constructor(torrent, f) {
    super();
    this.torrent = torrent;
    this.index = f.index;
    this.path = f.name;
    this.name = path.basename(f.name);
    this.length = f.size;
    this.offset = 0;
    this.update(f);
  }
  update(f) {
    this.progress = f.progress;
    this.priority = f.priority;
    this.downloaded = Math.round(f.size * f.progress);
  }
  get done() { return this.progress >= 1; }
  // İndirme sürerken okuma: istenen parça inene kadar bekler (sıralı indirme açılır).
  createReadStream({ start = 0, end = this.length - 1 } = {}) {
    const file = this;
    const t = this.torrent;
    const full = path.join(t.savePath, this.path);
    async function* chunks() {
      await t.engine.streaming(t, file);
      let pos = start;
      const fh = await waitOpen(full, t);
      try {
        while (pos <= end) {
          const piece = Math.floor((file.offset + pos) / t.pieceSize);
          await t.engine.waitPiece(t, piece);
          const pieceEnd = (piece + 1) * t.pieceSize - file.offset - 1;
          const upto = Math.min(end, pieceEnd);
          const buf = Buffer.alloc(upto - pos + 1);
          const { bytesRead } = await fh.read(buf, 0, buf.length, pos);
          if (!bytesRead) throw new Error('Dosya okunamadı.');
          yield buf.subarray(0, bytesRead);
          pos += bytesRead;
        }
      } finally {
        await fh.close();
      }
    }
    return Readable.from(chunks());
  }
}

async function waitOpen(full, t) {
  for (;;) {
    try { return await fs.promises.open(full, 'r'); } catch (err) {
      if (err.code !== 'ENOENT' || t.destroyed) throw err;
      await sleep(300);
    }
  }
}

class QbTorrent extends EventEmitter {
  constructor(engine, { savePath, deselect }) {
    super();
    this.engine = engine;
    this.savePath = savePath;
    this.deselect = deselect;
    this.infoHash = null;
    this.name = null;
    this.files = [];
    this.pieces = [];
    this.pieceSize = 0;
    this.length = 0;
    this.progress = 0;
    this.downloaded = 0;
    this.downloadSpeed = 0;
    this.uploadSpeed = 0;
    this.uploaded = 0;
    this.numPeers = 0;
    this.timeRemaining = Infinity;
    this.torrentFile = null;
    this.done = false;
    this.destroyed = false;
    this.hasMetadata = false;
    this.ops = Promise.resolve();
  }
  // Aynı torrentin API istekleri sırayla gider (ör. seçim, başlatmadan önce).
  queue(fn) {
    this.ops = this.ops.then(() => (this.destroyed ? null : fn())).catch((err) => this.engine.fail(this, err));
    return this.ops;
  }
}

// qBittorrent 5 pause/resume yerine stop/start kullanır.
async function control(api, action, hashes) {
  const [old, now] = action === 'pause' ? ['pause', 'stop'] : ['resume', 'start'];
  try {
    await api.post(`torrents/${old}`, { hashes });
  } catch (err) {
    if (!/\b404\b/.test(err.message)) throw err;
    await api.post(`torrents/${now}`, { hashes });
  }
}

// Torznab/Jackett indirme bağlantıları çoğu zaman bir magnet adresine yönlendirir;
// fetch magnet'e gidemez ("fetch failed"). Yönlendirmeler elle izlenir, magnet'e
// varılırsa o kullanılır. Döner: { buf } ya da { magnet }.
export async function fetchTorrentSource(url, fetchImpl = fetch) {
  let current = url;
  for (let hop = 0; hop < 6; hop++) {
    let res;
    try {
      res = await fetchImpl(current, { redirect: 'manual', signal: AbortSignal.timeout(30000) });
    } catch (err) {
      throw new Error(`Error downloading torrent: ${err.cause?.code || err.cause?.message || err.message}`);
    }
    const location = res.headers.get('location');
    if (res.status >= 300 && res.status < 400 && location) {
      if (/^magnet:/i.test(location)) return { magnet: location };
      current = new URL(location, current).href;
      continue;
    }
    // 4xx kalıcıdır (süresi dolmuş bağlantı, yanlış anahtar); yeniden denemek boşuna.
    if (res.status >= 400 && res.status < 500) throw new Error(`Torrent bağlantısı açılamadı (HTTP ${res.status}); bağlantının süresi dolmuş ya da API anahtarı yanlış olabilir.`);
    if (!res.ok) throw new Error(`Error downloading torrent: HTTP ${res.status}`);
    return { buf: Buffer.from(await res.arrayBuffer()) };
  }
  throw new Error('Torrent bağlantısı çok fazla yönlendirme yaptı.');
}

// start(): { api, stop } döndürür; varsayılanı qBittorrent'i profileDir ile başlatmaktır.
export function createQbEngine({ profileDir, start = () => launchQbittorrent({ profileDir }), pollMs = 1000, fetch: fetchImpl = fetch } = {}) {
  let session = null; // Promise<{ api, stop }>
  let limits = { download: 0, upload: 0 };
  const live = new Set();
  let timer = null;
  let failures = 0;
  let closing = false;

  async function ensure() {
    if (closing) throw new Error('Uygulama kapanıyor.');
    session ??= (async () => {
      const s = await start();
      await s.api.post('torrents/createCategory', { category: CATEGORY, savePath: '' }).catch(() => {});
      // Önceki oturumdan kalanlar durdurulur; hangisinin süreceğine Compass karar verir.
      await control(s.api, 'pause', await categoryHashes(s.api)).catch(() => {});
      await applyLimits(s.api);
      return s;
    })();
    try {
      return await session;
    } catch (err) {
      session = null;
      throw err;
    }
  }

  async function categoryHashes(api) {
    const list = await api.get('torrents/info', { category: CATEGORY });
    return list.map((t) => t.hash).join('|') || 'none';
  }

  async function applyLimits(api) {
    await api.post('transfer/setDownloadLimit', { limit: String(limits.download) });
    await api.post('transfer/setUploadLimit', { limit: String(limits.upload) });
  }

  function fail(t, err) {
    if (t.destroyed) return;
    t.destroyed = true;
    live.delete(t);
    if (t.infoHash && t.added && session) session.then((s) => control(s.api, 'pause', t.infoHash)).catch(() => {});
    t.emit('error', err instanceof Error ? err : new Error(String(err)));
  }

  // add() sürerken bırakılan torrent: eklendikten sonra kararı uygula.
  async function release2(api, t) {
    if (t.forget) await api.post('torrents/delete', { hashes: t.infoHash, deleteFiles: 'false' });
    else await control(api, 'pause', t.infoHash);
  }

  // Motordan ayrılan torrent: forget ise qBittorrent listesinden silinir (dosyalar kalır),
  // değilse durdurulup bekletilir.
  function release(t, forget) {
    if (!t || t.destroyed) return;
    t.destroyed = true;
    t.forget = forget;
    live.delete(t);
    if (!t.infoHash || !t.added || !session) return; // ekleme sürüyorsa add() kendisi halleder
    session.then((s) => (forget ? s.api.post('torrents/delete', { hashes: t.infoHash, deleteFiles: 'false' }) : control(s.api, 'pause', t.infoHash))).catch(() => {});
  }

  function schedule() {
    if (timer || closing) return;
    timer = setTimeout(async () => {
      timer = null;
      try {
        await poll();
        failures = 0;
      } catch (err) {
        // qBittorrent kapandıysa canlı torrentler geçici hata alır; yeniden deneme yeniden başlatır.
        if (++failures >= 3) {
          failures = 0;
          session = null;
          for (const t of [...live]) fail(t, new Error(`connect ECONNREFUSED qBittorrent (${err.message})`));
        }
      }
      if (live.size) schedule();
    }, [...live].some((t) => !t.hasMetadata) ? Math.min(pollMs, 250) : pollMs); // dosya listesi gelene kadar sık bak
    timer.unref?.();
  }

  async function poll() {
    const tracked = [...live].filter((t) => t.infoHash && t.added);
    if (!tracked.length) return;
    const { api } = await session;
    const infos = new Map((await api.get('torrents/info', { category: CATEGORY })).map((i) => [i.hash, i]));
    for (const t of tracked) {
      if (t.destroyed) continue;
      const info = infos.get(t.infoHash);
      if (!info) { fail(t, new Error('Torrent qBittorrent listesinden kaldırıldı.')); continue; }
      if (info.state === 'error') { fail(t, new Error('qBittorrent bu torrentte hata bildirdi (klasöre yazılamıyor ya da disk dolu olabilir).')); continue; }
      if (info.state === 'missingFiles') { fail(t, new Error('İndirilen dosyalar bulunamadı; taşınmış ya da silinmiş olabilir.')); continue; }
      t.name = info.name;
      t.downloadSpeed = info.dlspeed;
      t.uploadSpeed = info.upspeed;
      t.uploaded = info.uploaded_session;
      t.numPeers = info.num_seeds + info.num_leechs;
      t.timeRemaining = info.eta >= NO_ETA ? Infinity : info.eta * 1000;
      if (!t.hasMetadata) {
        if (info.state === 'metaDL' || !info.total_size) continue;
        await loadMetadata(api, t, info);
        continue;
      }
      const files = await api.get('torrents/files', { hash: t.infoHash });
      for (const f of files) t.files[f.index]?.update(f);
      t.length = info.total_size;
      t.downloaded = info.completed;
      t.progress = info.progress;
      if (BUSY.has(info.state)) continue;
      for (const f of t.files) {
        if (f.priority > 0 && f.done && !f.reported) { f.reported = true; f.emit('done'); }
      }
      const wanted = t.files.filter((f) => f.priority > 0);
      if (!t.done && wanted.length && wanted.every((f) => f.done)) {
        t.done = true;
        t.emit('done');
      }
    }
  }

  async function loadMetadata(api, t, info) {
    const [files, props] = await Promise.all([api.get('torrents/files', { hash: t.infoHash }), api.get('torrents/properties', { hash: t.infoHash })]);
    if (t.destroyed) return;
    files.sort((a, b) => a.index - b.index);
    let offset = 0;
    t.files = files.map((f) => {
      const file = new QbFile(t, f);
      file.offset = offset;
      offset += f.size;
      return file;
    });
    t.pieceSize = props.piece_size;
    t.pieces = new Array(Math.ceil(info.total_size / props.piece_size));
    t.length = info.total_size;
    t.name = info.name;
    try { t.torrentFile = await api.get('torrents/export', { hash: t.infoHash }, { raw: true }); } catch { /* eski qBittorrent */ }
    // Bekletilen torrent eski seçimini taşıyabilir; varsayılan her eklemede açıkça kurulur.
    const want = t.deselect ? 0 : 1;
    await setPriority(api, t, t.files.filter((f) => f.priority !== want).map((f) => f.index), want);
    if (t.destroyed) return;
    t.hasMetadata = true;
    t.emit('metadata');
    t.emit('ready');
    // Yönetici 'metadata'da dosya seçebilir ya da torrenti kapatabilir (seçim penceresi,
    // .exe engeli). Başlatma onun kararlarından sonra sıraya girer.
    setImmediate(() => t.queue(async () => {
      // Altyazılar küçüktür ve oynatıcı onları hemen ister: seçiliyse önce insin.
      const subs = t.files.filter((f) => f.priority > 0 && f.length <= 1024 * 1024 && SUBTITLE.test(f.name)).map((f) => f.index);
      await setPriority(api, t, subs, 7);
      await control(api, 'resume', t.infoHash);
    }));
  }

  async function setPriority(api, t, indexes, priority) {
    if (!indexes.length) return;
    for (const i of indexes) if (t.files[i]) t.files[i].priority = priority;
    await api.post('torrents/filePrio', { hash: t.infoHash, id: indexes.join('|'), priority: String(priority) });
  }

  function withApi(t, fn) {
    t.queue(async () => fn((await ensure()).api));
  }

  return {
    add(source, { path: savePath, deselect = false }) {
      const t = new QbTorrent(this, { savePath, deselect });
      live.add(t);
      t.queue(async () => {
        const { api } = await ensure();
        let buf = null;
        let magnet = null;
        if (Buffer.isBuffer(source)) buf = source;
        else if (/^magnet:/i.test(source)) magnet = source;
        else ({ buf = null, magnet = null } = await fetchTorrentSource(source, fetchImpl));
        const parsed = await parseTorrent(buf || magnet);
        t.infoHash = parsed.infoHash;
        if (buf) t.torrentFile = buf;
        if (t.destroyed) return;
        // Duraklatılıp bekletilen torrent yeniden eklenmez: dosyalar baştan kontrol edilmeden sürer.
        if ((await api.get('torrents/info', { hashes: t.infoHash })).length) {
          await control(api, 'pause', t.infoHash); // seçim yeniden yapılana kadar inmesin
          t.added = true;
          if (t.destroyed) release2(api, t);
          else schedule();
          return;
        }
        fs.mkdirSync(savePath, { recursive: true });
        const form = new FormData();
        form.append('savepath', savePath);
        form.append('category', CATEGORY);
        form.append('contentLayout', 'Original');
        if (buf) {
          form.append('torrents', new Blob([buf]), 'item.torrent');
          form.append('paused', 'true');
          form.append('stopped', 'true'); // qBittorrent 5
        } else {
          form.append('urls', magnet);
          form.append('stopCondition', 'MetadataReceived');
        }
        const reply = await api.post('torrents/add', form);
        if (!/ok/i.test(String(reply))) throw new Error('qBittorrent torrenti eklemedi.');
        t.added = true;
        if (t.destroyed) await release2(api, t);
        else schedule();
      });
      return t;
    },
    // Duraklatma: torrent qBittorrent'te durur; devam etmek aynı torrenti yeniden üstlenmektir.
    pause(t) {
      release(t, false);
    },
    // Tamamen bırak: qBittorrent listesinden silinir, dosyalar diskte kalır.
    remove(t) {
      release(t, true);
    },
    resume(source, opts) {
      return this.add(source, opts);
    },
    selectFiles(t, indexes) {
      withApi(t, (api) => setPriority(api, t, indexes, 1));
    },
    deselectAll(t) {
      withApi(t, (api) => setPriority(api, t, t.files.map((f) => f.index), 0));
    },
    selectAll(t) {
      withApi(t, (api) => setPriority(api, t, t.files.map((f) => f.index), 1));
    },
    throttle({ download = 0, upload = 0 } = {}) {
      limits = { download: Math.max(0, Number(download) || 0), upload: Math.max(0, Number(upload) || 0) };
      if (session) session.then((s) => applyLimits(s.api)).catch(() => {});
    },
    // İndirirken izleme: okunan dosya en yüksek önceliği alır, torrent sırayla iner,
    // ilk/son parçalar öne alınır (oynatıcı dosyanın sonundaki bilgiyi de okur).
    async streaming(t, file) {
      const { api } = await ensure();
      if (file.priority > 0 && file.priority < 7 && !file.done) await setPriority(api, t, [file.index], 7);
      if (t.sequential) return;
      t.sequential = true;
      const info = (await api.get('torrents/info', { hashes: t.infoHash }))[0];
      if (info && !info.seq_dl) await api.post('torrents/toggleSequentialDownload', { hashes: t.infoHash });
      if (info && !info.f_l_piece_prio) await api.post('torrents/toggleFirstLastPiecePrio', { hashes: t.infoHash });
    },
    async waitPiece(t, piece) {
      const { api } = await ensure();
      for (;;) {
        if (t.destroyed) throw new Error('İndirme durduruldu.');
        if (!t.havePieces || t.havePieces[piece] !== 2) t.havePieces = await api.get('torrents/pieceStates', { hash: t.infoHash });
        if (t.havePieces[piece] === 2) return;
        await sleep(400);
      }
    },
    fail,
    async destroy() {
      closing = true;
      clearTimeout(timer);
      for (const t of live) t.destroyed = true;
      live.clear();
      if (!session) return;
      const s = await session.catch(() => null);
      session = null;
      if (!s) return;
      await s.stop?.();
    },
  };
}
