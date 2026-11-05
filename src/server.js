import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import crypto from 'node:crypto';
import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createSettingsStore } from './settings.js';
import { createDownloadManager, subtitleToVtt, MAX_SUBTITLE_BYTES } from './downloads.js';
import { createQbEngine } from './qbittorrent.js';
import { activeProviders, searchAll } from './search.js';
import { createLibrary } from './library.js';
import { createRatingsCache, addImdbRatings } from './ratings.js';
import { CATEGORIES, GENRES, detectGenres } from './categories.js';

const MASK = '••••••';
const PUBLIC_DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), '..', 'public');
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.ico': 'image/x-icon',
};

export function defaultDataDir() {
  const home = os.homedir();
  const base = process.platform === 'win32' ? process.env.APPDATA || home
    : process.platform === 'darwin' ? path.join(home, 'Library', 'Application Support')
      : process.env.XDG_CONFIG_HOME || path.join(home, '.config');
  // Eski adla ("Torrent Bul") kaydedilmiş veriler varsa onlar kullanılır.
  const legacy = path.join(base, process.platform === 'linux' ? 'torrent-bul' : 'Torrent Bul');
  if (fs.existsSync(legacy)) return legacy;
  return path.join(base, process.platform === 'linux' ? 'compass' : 'Compass');
}

function systemOpen(target) {
  const cmd = process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'explorer' : 'xdg-open';
  spawn(cmd, [target], { detached: true, stdio: 'ignore' }).on('error', () => {}).unref();
}


function send(res, status, body) {
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' });
  res.end(JSON.stringify(body));
}

async function readJson(req) {
  let raw = '';
  for await (const chunk of req) {
    raw += chunk;
    if (raw.length > 25e6) throw new Error('İstek çok büyük.');
  }
  return raw ? JSON.parse(raw) : {};
}

export async function startServer({
  port = 0,
  host = '127.0.0.1',
  dataDir = defaultDataDir(),
  openPath = systemOpen,
  engine,
  fetchImpl = fetch,
} = {}) {
  const settings = createSettingsStore(path.join(dataDir, 'settings.json'));
  const library = createLibrary({ file: path.join(dataDir, 'library.json') });
  const ratingsCache = createRatingsCache(path.join(dataDir, 'omdb-cache.json'));
  const downloads = createDownloadManager({
    stateFile: path.join(dataDir, 'downloads.json'),
    getDownloadDir: () => settings.get().downloadDir,
    shouldSeed: () => settings.get().seedAfterDone === true,
    engine: engine || createQbEngine({ profileDir: path.join(dataDir, 'qbittorrent') }),
    onDone: (info) => library.upsert(info),
    maxActive: () => settings.get().maxActiveDownloads,
  });
  const applyLimits = () => downloads.throttle({
    download: settings.get().downloadLimitKbps * 1024,
    upload: settings.get().uploadLimitKbps * 1024,
  });
  applyLimits();
  // Kütüphaneden önceki sürümlerde bitmiş indirmeler de kütüphaneye girsin.
  for (const info of downloads.completed()) if (!library.has(info.infoHash)) library.upsert(info);
  // Yalnızca bu uygulamanın kendi sayfası API'yi çağırabilsin diye oturum anahtarı.
  const token = crypto.randomBytes(24).toString('hex');

  const publicSettings = () => {
    const s = settings.get();
    return {
      ...s,
      omdbApiKey: s.omdbApiKey ? MASK : '',
      torznab: s.torznab.map((t) => ({ ...t, apiKey: t.apiKey ? MASK : '' })),
    };
  };

  async function api(req, res, url) {
    const parts = url.pathname.split('/').filter(Boolean); // ['api', ...]
    const route = `${req.method} /${parts.slice(1).join('/')}`;

    if (route === 'GET /meta') {
      return send(res, 200, {
        categories: Object.fromEntries(Object.entries(CATEGORIES).map(([k, v]) => [k, v.label])),
        genres: Object.fromEntries(Object.entries(GENRES).map(([k, v]) => [k, v.label])),
        imdb: Boolean(settings.get().omdbApiKey),
        providers: activeProviders(settings.get()).map((p) => ({ id: p.id, name: p.name })),
      });
    }
    if (route === 'GET /search') {
      const params = {
        query: (url.searchParams.get('q') || '').slice(0, 200),
        category: url.searchParams.get('category') || 'all',
        genre: url.searchParams.get('genre') || '',
      };
      if (!params.query.trim() && !params.genre && params.category !== 'adult') return send(res, 400, { error: 'Bir şey yazın ya da bir tür seçin.' });
      const providers = activeProviders(settings.get());
      if (!providers.length) return send(res, 400, { error: 'Etkin arama kaynağı yok. Ayarlar’dan bir kaynak açın.' });
      return send(res, 200, await searchAll(params, providers, { fetch: fetchImpl, omdbApiKey: settings.get().omdbApiKey, ratingsCache }));
    }
    // Aramada süreye sığmayan afiş ve puanlar: arayüz sonuçları gösterdikten sonra sorar.
    if (route === 'POST /ratings') {
      const body = await readJson(req);
      const items = (Array.isArray(body.items) ? body.items : []).slice(0, 40)
        .filter((r) => r && typeof r.id === 'string' && typeof r.title === 'string' && (r.category === 'movies' || r.category === 'tv'))
        .map((r) => ({
          id: r.id,
          title: r.title.slice(0, 300),
          year: Number.isInteger(r.year) ? r.year : null,
          category: r.category,
          imdbId: /^tt\d{5,10}$/.test(r.imdbId || '') ? r.imdbId : undefined,
        }));
      await addImdbRatings(items, settings.get().omdbApiKey, { fetch: fetchImpl, ratingsCache, budgetMs: 8000 });
      const out = {};
      for (const r of items) {
        if (r.imdbUrl) out[r.id] = { rating: r.rating, poster: r.poster, imdbUrl: r.imdbUrl, year: r.year, genres: detectGenres(r.subjects || []) };
      }
      return send(res, 200, { ratings: out });
    }
    if (route === 'GET /settings') return send(res, 200, publicSettings());
    if (route === 'PUT /settings') {
      const body = await readJson(req);
      // Maskelenmiş anahtar geri gelirse eskisini koru.
      if (Array.isArray(body.torznab)) {
        const old = settings.get().torznab;
        body.torznab = body.torznab.map((t) => {
          if (t.apiKey !== MASK) return t;
          const prev = old.find((o) => o.url === t.url);
          return { ...t, apiKey: prev?.apiKey || '' };
        });
      }
      if (body.omdbApiKey === MASK) delete body.omdbApiKey;
      const seedBefore = settings.get().seedAfterDone;
      settings.update(body);
      if (settings.get().seedAfterDone !== seedBefore) downloads.setSeeding(settings.get().seedAfterDone);
      applyLimits();
      downloads.pump();
      return send(res, 200, publicSettings());
    }
    if (route === 'GET /downloads') return send(res, 200, downloads.list());
    if (route === 'GET /library') return send(res, 200, library.list());
    // Kullanıcının seçtiği yerel altyazıyı yalnızca VTT'ye çevirir; diske yazılmaz.
    if (route === 'POST /subtitles/preview') {
      const body = await readJson(req);
      const name = String(body.name || 'altyazi.srt');
      const buf = Buffer.from(String(body.data || ''), 'base64');
      if (!/\.(srt|vtt)$/i.test(name)) return send(res, 400, { error: 'Yalnızca .srt ya da .vtt dosyası.' });
      if (!buf.length) return send(res, 400, { error: 'Altyazı dosyası boş.' });
      if (buf.length > MAX_SUBTITLE_BYTES) return send(res, 400, { error: 'Altyazı dosyası 1 MB’tan büyük olamaz.' });
      return send(res, 200, { name, vtt: subtitleToVtt(buf, name) });
    }
    if (req.method === 'PUT' && parts[1] === 'library' && parts[3] === 'progress' && !parts[4]) {
      const out = library.progress(parts[2], await readJson(req));
      return out ? send(res, 200, out) : send(res, 404, { error: 'Kütüphanede yok.' });
    }
    if (route === 'POST /downloads') {
      const body = await readJson(req);
      return send(res, 201, await downloads.add(body));
    }
    if (route === 'POST /torrents/inspect') {
      const body = await readJson(req);
      const files = Array.isArray(body.files) ? body.files.slice(0, 500) : [];
      const out = await Promise.all(files.map(async (f) => {
        try {
          return { file: String(f.name || ''), ...(await downloads.inspect(f.data)) };
        } catch (err) {
          return { file: String(f.name || ''), error: err.message };
        }
      }));
      return send(res, 200, out);
    }
    if (parts[1] === 'downloads' && parts[2]) {
      const id = parts[2];
      const action = parts[3];
      let out;
      if (req.method === 'GET' && action === 'files') out = downloads.files(id);
      else if (req.method === 'POST' && action === 'files') out = downloads.selectFiles(id, (await readJson(req)).indexes);
      else if (req.method === 'POST' && action === 'pause') out = downloads.pause(id);
      else if (req.method === 'POST' && action === 'resume') out = downloads.resume(id);
      else if (req.method === 'POST' && action === 'open-file') {
        const file = downloads.openFile(id, Number((await readJson(req)).index));
        if (file?.path) openPath(file.path);
        out = file?.path ? { ok: true } : null;
      } else if (req.method === 'POST' && action === 'open') {
        const folder = downloads.folderOf(id);
        if (folder) openPath(folder);
        out = folder ? { ok: true } : null;
      } else if (req.method === 'DELETE' && !action) out = downloads.remove(id) ? { ok: true } : null;
      else return send(res, 405, { error: 'Desteklenmeyen işlem.' });
      return out ? send(res, 200, out) : send(res, 404, { error: 'İndirme bulunamadı.' });
    }
    if (route === 'POST /open-download-dir') {
      const dir = settings.get().downloadDir;
      fs.mkdirSync(dir, { recursive: true });
      openPath(dir);
      return send(res, 200, { ok: true });
    }
    return send(res, 404, { error: 'Bulunamadı.' });
  }

  function serveStatic(req, res, url) {
    const rel = url.pathname === '/' ? 'index.html' : decodeURIComponent(url.pathname).replace(/^\/+/, '');
    const file = path.normalize(path.join(PUBLIC_DIR, rel));
    if (!file.startsWith(PUBLIC_DIR + path.sep)) return send(res, 403, { error: 'Yasak.' });
    fs.readFile(file, (err, buf) => {
      if (err) return send(res, 404, { error: 'Bulunamadı.' });
      let body = buf;
      if (rel === 'index.html') body = buf.toString('utf8').replace('__APP_TOKEN__', token);
      res.writeHead(200, {
        'Content-Type': MIME[path.extname(file)] || 'application/octet-stream',
        'Cache-Control': 'no-store',
        'X-Content-Type-Options': 'nosniff',
      });
      res.end(body);
    });
  }

  // <video> ve <track> özel başlık gönderemediği için anahtar adreste taşınır.
  async function serveMedia(req, res, url) {
    const [, , id, index, extra] = url.pathname.split('/'); // /media/:id/:index[/subtitle.vtt]
    if (url.searchParams.get('t') !== token) return send(res, 403, { error: 'Yetkisiz istek.' });
    if (extra === 'subtitle.vtt') {
      const sub = id.startsWith('lib-') ? null : await downloads.subtitleFor(id, Number(index)).catch(() => null);
      if (!sub) return send(res, 404, { error: 'Altyazı yok.' });
      res.writeHead(200, { 'Content-Type': 'text/vtt; charset=utf-8', 'Cache-Control': 'no-store' });
      return res.end(sub.vtt);
    }
    // "lib-<infohash>": indirme listeden kaldırılmış ama dosya kütüphanede duruyor.
    const file = id.startsWith('lib-') ? library.openFile(id.slice(4), Number(index)) : downloads.openFile(id, Number(index));
    if (!file) return send(res, 404, { error: 'Dosya bulunamadı. Taşınmış ya da silinmiş olabilir.' });
    const headers = { 'Content-Type': file.type, 'Accept-Ranges': 'bytes', 'Cache-Control': 'no-store' };
    const m = /^bytes=(\d*)-(\d*)$/.exec(req.headers.range || '');
    let start = 0;
    let end = file.size - 1;
    if (m && (m[1] || m[2])) {
      if (m[1]) { start = Number(m[1]); if (m[2]) end = Math.min(Number(m[2]), end); }
      else start = Math.max(0, file.size - Number(m[2])); // "bytes=-500": son 500 bayt
      if (start > end || start >= file.size) {
        res.writeHead(416, { 'Content-Range': `bytes */${file.size}` });
        return res.end();
      }
      res.writeHead(206, { ...headers, 'Content-Range': `bytes ${start}-${end}/${file.size}`, 'Content-Length': end - start + 1 });
    } else {
      res.writeHead(200, { ...headers, 'Content-Length': file.size });
    }
    if (req.method === 'HEAD') return res.end();
    const stream = file.stream({ start, end });
    stream.on('error', () => res.destroy());
    res.on('close', () => stream.destroy());
    stream.pipe(res);
  }

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, `http://${req.headers.host || 'localhost'}`);
    if (url.pathname.startsWith('/media/')) return serveMedia(req, res, url).catch(() => res.destroy());
    if (!url.pathname.startsWith('/api/')) return serveStatic(req, res, url);
    if (req.headers['x-app-token'] !== token) return send(res, 403, { error: 'Yetkisiz istek.' });
    try {
      await api(req, res, url);
    } catch (err) {
      send(res, 400, { error: err.message || 'Beklenmeyen hata.' });
    }
  });

  await new Promise((resolve) => server.listen(port, host, resolve));
  const address = server.address();
  return {
    url: `http://${host}:${address.port}/`,
    token,
    getSettings: () => settings.get(),
    // Masaüstü uygulaması (ör. işletim sisteminden gelen magnet) için.
    addDownload: (body) => downloads.add(body),
    async close() {
      await downloads.close();
      library.close();
      await new Promise((r) => server.close(() => r()));
    },
  };
}
