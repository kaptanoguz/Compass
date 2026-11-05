import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-pick-'));
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

const fill = (size, seed) => {
  const b = Buffer.alloc(size);
  for (let i = 0; i < size; i++) b[i] = (i * seed) % 251;
  return b;
};

// Yerel web-seed: dosyaları ve .torrent'i HTTP ile sunar. "gate" açılana kadar
// dosya istekleri bekletilir; böylece indirme yarıdayken uygulama kapatılabilir.
async function fixture(files, name = 'Paket') {
  let release;
  const gate = { open: Promise.resolve(), hold() { this.open = new Promise((r) => { release = r; }); }, release: () => release?.() };
  const torrents = {};
  const server = http.createServer(async (req, res) => {
    if (torrents[req.url]) { res.writeHead(200); return res.end(torrents[req.url]); }
    await gate.open;
    const rel = decodeURIComponent(req.url).replace(`/${name}/`, '');
    // Tek dosyalı torrentte web-seed adresi doğrudan dosyanın kendisidir.
    const body = Object.keys(files).length === 1 ? Object.values(files)[0] : files[rel];
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
  const single = input.length === 1;
  const buf = Buffer.from(await new Promise((resolve, reject) =>
    createTorrent(single ? input[0] : input, { name: single ? input[0].name : name, pieceLength: 16384, urlList: [`${base}/`], announceList: [] },
      (err, t) => (err ? reject(err) : resolve(t)))));
  torrents['/item.torrent'] = buf;
  return { server, base, gate, torrent: buf, url: `${base}/item.torrent` };
}

const fullyOnDisk = (file, content) => fs.existsSync(file) && fs.readFileSync(file).equals(content);

test('çok dosyalı torrentte yalnızca seçilen dosya iner, seçim yeniden açılışta durur', async () => {
  const film = fill(128 * 1024, 7);
  const ornek = fill(64 * 1024, 11);
  const { server, url } = await fixture({ 'film.mp4': film, 'sample.mp4': ornek, 'film.tr.srt': Buffer.from('1\n00:00:01,000 --> 00:00:02,000\nMerhaba\n') });
  const dataDir = tmp();
  const downloadDir = path.join(dataDir, 'dl');
  let app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir }) });
    const { body: added } = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: url, title: 'Paket' }) });
    const [picking] = await waitFor(app, (l) => l[0]?.status === 'picking');
    assert.equal(picking.fileCount, 3);

    const list = await call(app, `downloads/${added.id}/files`);
    assert.equal(list.status, 200);
    assert.deepEqual(list.body.files.map((f) => [f.name, f.kind, f.length]), [
      ['film.mp4', 'video', film.length], ['sample.mp4', 'video', ornek.length], ['film.tr.srt', 'subtitle', 40],
    ]);
    // Seçim beklerken hiçbir şey inmez.
    await wait(300);
    assert.ok(!fullyOnDisk(path.join(downloadDir, 'Paket', 'film.mp4'), film));

    assert.equal((await call(app, `downloads/${added.id}/files`, { method: 'POST', body: JSON.stringify({ indexes: [] }) })).status, 400);
    assert.equal((await call(app, `downloads/${added.id}/files`, { method: 'POST', body: JSON.stringify({ indexes: [9] }) })).status, 400);
    const chosen = await call(app, `downloads/${added.id}/files`, { method: 'POST', body: JSON.stringify({ indexes: [0, 2] }) });
    assert.equal(chosen.status, 200);
    assert.equal(chosen.body.status, 'downloading');

    const [done] = await waitFor(app, (l) => l[0]?.status === 'done');
    assert.equal(done.missing, false);
    assert.deepEqual(done.videos.map((v) => v.name), ['film.mp4']);
    assert.ok(fullyOnDisk(path.join(downloadDir, 'Paket', 'film.mp4'), film));
    assert.ok(!fullyOnDisk(path.join(downloadDir, 'Paket', 'sample.mp4'), ornek), 'seçilmeyen dosya inmedi');
    const saved = JSON.parse(fs.readFileSync(path.join(dataDir, 'downloads.json'), 'utf8'));
    assert.deepEqual(saved[0].selectedIndexes, [0, 2]);

    await app.close();
    app = await startServer({ dataDir, openPath: () => {} });
    const again = await call(app, `downloads/${added.id}/files`);
    assert.deepEqual(again.body.files.map((f) => f.selected), [true, false, true]);
    await wait(500);
    assert.equal((await call(app, 'downloads')).body[0].status, 'done');
    assert.ok(!fullyOnDisk(path.join(downloadDir, 'Paket', 'sample.mp4'), ornek));
  } finally {
    await app.close();
    await new Promise((r) => server.close(r));
  }
});

test('yarıda kapanan seçimli indirme açılınca yalnızca seçilen dosyayla sürer', async () => {
  const a = fill(64 * 1024, 3);
  const b = fill(96 * 1024, 5);
  const { server, gate, torrent } = await fixture({ 'bolum1.mkv': a, 'bolum2.mkv': b }, 'Dizi');
  const dataDir = tmp();
  const downloadDir = path.join(dataDir, 'dl');
  let app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir }) });
    const inspected = await call(app, 'torrents/inspect', { method: 'POST', body: JSON.stringify({ files: [{ name: 'dizi.torrent', data: torrent.toString('base64') }] }) });
    assert.deepEqual(inspected.body[0].files.map((f) => [f.index, f.name, f.kind]), [[0, 'bolum1.mkv', 'video'], [1, 'bolum2.mkv', 'video']]);

    gate.hold();
    const added = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: torrent.toString('base64'), fileIndexes: [1] }) });
    assert.equal(added.status, 201);
    assert.notEqual(added.body.status, 'picking');
    await app.close();

    app = await startServer({ dataDir, openPath: () => {} });
    gate.release();
    const [done] = await waitFor(app, (l) => l[0]?.status === 'done');
    assert.equal(done.length, b.length);
    assert.ok(fullyOnDisk(path.join(downloadDir, 'Dizi', 'bolum2.mkv'), b));
    assert.ok(!fullyOnDisk(path.join(downloadDir, 'Dizi', 'bolum1.mkv'), a), 'seçilmeyen bölüm inmedi');
  } finally {
    gate.release();
    await app.close();
    await new Promise((r) => server.close(r));
  }
});

test('tek dosyalı torrent seçim sormadan doğrudan iner', async () => {
  const film = fill(80 * 1024, 13);
  const { server, url } = await fixture({ 'tek.mp4': film });
  const dataDir = tmp();
  const app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir: path.join(dataDir, 'dl') }) });
    await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: url }) });
    const seen = new Set();
    const [done] = await waitFor(app, (l) => { seen.add(l[0]?.status); return l[0]?.status === 'done'; });
    assert.ok(!seen.has('picking'));
    assert.equal(done.fileCount, 1);
    assert.ok(fullyOnDisk(path.join(dataDir, 'dl', 'tek.mp4'), film));
  } finally {
    await app.close();
    await new Promise((r) => server.close(r));
  }
});
