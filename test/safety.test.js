import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';
import { screenPrograms, BLOCKED_MESSAGE } from '../src/downloads.js';
import { mapDoc } from '../src/providers/archive.js';
import { createLibrary } from '../src/library.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-safe-'));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function call(app, p, opts = {}) {
  const res = await fetch(new URL(`api/${p}`, app.url), { ...opts, headers: { 'X-App-Token': app.token, 'Content-Type': 'application/json' } });
  return { status: res.status, body: await res.json() };
}

async function waitFor(app, check) {
  for (let i = 0; i < 100; i++) {
    const list = (await call(app, 'downloads')).body;
    if (check(list)) return list;
    await wait(100);
  }
  throw new Error('zaman aşımı');
}

// Yerel web-seed: çok dosyalı torrent ve .torrent dosyası.
async function fixture(files, name = 'Paket') {
  const server = http.createServer((req, res) => {
    if (req.url === '/item.torrent') { res.writeHead(200); return res.end(torrent); }
    const body = files[decodeURIComponent(req.url).replace(`/${name}/`, '')];
    if (!body) { res.writeHead(404); return res.end(); }
    const m = /bytes=(\d+)-(\d+)?/.exec(req.headers.range || '');
    const start = m ? Number(m[1]) : 0;
    const end = m && m[2] ? Number(m[2]) : body.length - 1;
    res.writeHead(m ? 206 : 200, { 'Content-Length': end - start + 1, ...(m ? { 'Content-Range': `bytes ${start}-${end}/${body.length}` } : {}) });
    res.end(body.subarray(start, end + 1));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  const input = Object.entries(files).map(([n, buf]) => Object.assign(Buffer.from(buf), { name: n }));
  const torrent = Buffer.from(await new Promise((resolve, reject) =>
    createTorrent(input, { name, pieceLength: 16384, urlList: [`${base}/`], announceList: [] }, (e, t) => (e ? reject(e) : resolve(t)))));
  return { server, torrent, url: `${base}/item.torrent` };
}

async function withApp(fn) {
  const dataDir = tmp();
  const app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir: path.join(dataDir, 'dl') }) });
    await fn(app, path.join(dataDir, 'dl'));
  } finally {
    await app.close();
  }
}

test('program dosyaları ayıklanır, film yoksa torrent sahte sayılır', () => {
  assert.equal(screenPrograms(['Film.mkv', 'Film.srt']), null);
  assert.deepEqual(screenPrograms(['Film.mkv', 'Codec.exe', 'Film.srt']), { keep: [0, 2], skipped: 1 });
  assert.deepEqual(screenPrograms(['Film.2024.1080p.mp4.exe', 'Film.nfo', 'Okubeni.txt']), { blocked: true });
  assert.deepEqual(screenPrograms(['setup.msi']), { blocked: true });
  assert.deepEqual(screenPrograms(['Album.flac', 'kur.bat']), { keep: [0], skipped: 1 });
});

test('film diye gelen .exe hiç indirilmez', async () => {
  const exe = Buffer.alloc(64 * 1024, 7);
  const { server, url } = await fixture({ 'Film.2024.1080p.mp4.exe': exe, 'Film.nfo': Buffer.from('nfo') });
  try {
    await withApp(async (app, dl) => {
      await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: url, title: 'Film 2024', category: 'movies' }) });
      const [d] = await waitFor(app, (l) => l[0]?.status === 'error');
      assert.equal(d.error, BLOCKED_MESSAGE);
      await wait(300);
      const file = path.join(dl, 'Paket', 'Film.2024.1080p.mp4.exe');
      assert.ok(!fs.existsSync(file) || !fs.readFileSync(file).equals(exe), 'exe inmedi');
    });
  } finally {
    await new Promise((r) => server.close(r));
  }
});

test('filmin yanındaki .exe atlanır, yalnızca film iner', async () => {
  const film = Buffer.alloc(96 * 1024, 3);
  const exe = Buffer.alloc(64 * 1024, 9);
  const { server, url } = await fixture({ 'Film.mp4': film, 'Codec.exe': exe });
  try {
    await withApp(async (app, dl) => {
      await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: url, category: 'movies' }) });
      const seen = new Set();
      const [d] = await waitFor(app, (l) => { seen.add(l[0]?.status); return l[0]?.status === 'done'; });
      assert.ok(!seen.has('picking'), 'tek film kalınca seçim sorulmaz');
      assert.equal(d.skippedPrograms, 1);
      assert.ok(fs.readFileSync(path.join(dl, 'Paket', 'Film.mp4')).equals(film));
      const exePath = path.join(dl, 'Paket', 'Codec.exe');
      assert.ok(!fs.existsSync(exePath) || !fs.readFileSync(exePath).equals(exe), 'exe inmedi');
    });
  } finally {
    await new Promise((r) => server.close(r));
  }
});

test('dosya seçiminde program seçilemez; .torrent ile sahte film eklenmez; Yazılım kategorisi serbest', async () => {
  const { server, url } = await fixture({ 'Film.mp4': Buffer.alloc(64 * 1024, 1), 'Film.srt': Buffer.from('1\n'), 'Codec.exe': Buffer.alloc(1000, 2) });
  const fake = await fixture({ 'Film.2024.mkv.exe': Buffer.alloc(32 * 1024, 4), 'Film.nfo': Buffer.from('nfo') }, 'Sahte');
  try {
    await withApp(async (app) => {
      const { body: added } = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: url, category: 'movies' }) });
      await waitFor(app, (l) => l[0]?.status === 'picking');
      const files = (await call(app, `downloads/${added.id}/files`)).body.files;
      assert.equal(files.find((f) => f.name === 'Codec.exe').kind, 'program');
      const bad = await call(app, `downloads/${added.id}/files`, { method: 'POST', body: JSON.stringify({ indexes: [0, 2] }) });
      assert.equal(bad.status, 400);
      assert.equal((await call(app, `downloads/${added.id}/files`, { method: 'POST', body: JSON.stringify({ indexes: [0, 1] }) })).status, 200);

      const data = fake.torrent.toString('base64');
      const inspected = (await call(app, 'torrents/inspect', { method: 'POST', body: JSON.stringify({ files: [{ name: 'sahte.torrent', data }] }) })).body[0];
      assert.equal(inspected.blocked, true);
      const refused = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: data }) });
      assert.equal(refused.status, 400);
      assert.equal(refused.body.error, BLOCKED_MESSAGE);
      const software = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: data, category: 'software' }) });
      assert.equal(software.status, 201);
    });
  } finally {
    await new Promise((r) => server.close(r));
    await new Promise((r) => fake.server.close(r));
  }
});

test('Archive sonuçlarında ve kütüphanede afiş olur', () => {
  assert.equal(mapDoc({ identifier: 'nosferatu' }, 0, 1).poster, 'https://archive.org/services/img/nosferatu');
  const lib = createLibrary({ file: path.join(tmp(), 'library.json') });
  // Afişsiz eklenmiş eski bir Archive kaydı
  lib.upsert({ infoHash: 'b'.repeat(40), title: 'Metropolis', pageUrl: 'https://archive.org/details/Metropolis1927', downloadId: 'x', files: [] });
  assert.equal(lib.list()[0].poster, 'https://archive.org/services/img/Metropolis1927');
});
