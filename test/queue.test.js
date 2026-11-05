import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';
import { createQbEngine } from '../src/qbittorrent.js';
import { createSettingsStore } from '../src/settings.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-queue-'));
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

test('limit 1 iken ikinci indirme sırada bekler, birincisi bitince başlar', async () => {
  const a = Buffer.alloc(64 * 1024, 1);
  const b = Buffer.alloc(64 * 1024, 2);
  const bodies = { '/bir.mp4': a, '/iki.mp4': b };
  let open;
  let gate = new Promise((r) => { open = r; });
  const server = http.createServer(async (req, res) => {
    await gate; // birinci indirme biz bırakana kadar sürsün
    const body = bodies[req.url];
    if (!body) { res.writeHead(404); return res.end(); }
    const m = /bytes=(\d+)-(\d+)?/.exec(req.headers.range || '');
    const start = m ? Number(m[1]) : 0;
    const end = m && m[2] ? Number(m[2]) : body.length - 1;
    res.writeHead(m ? 206 : 200, { 'Content-Length': end - start + 1, ...(m ? { 'Content-Range': `bytes ${start}-${end}/${body.length}` } : {}) });
    res.end(body.subarray(start, end + 1));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  const make = (name, buf) => new Promise((resolve, reject) =>
    createTorrent(Object.assign(Buffer.from(buf), { name }), { name, urlList: [`${base}/${name}`], announceList: [] }, (e, t) => (e ? reject(e) : resolve(Buffer.from(t)))));
  const t1 = await make('bir.mp4', a);
  const t2 = await make('iki.mp4', b);

  const dataDir = tmp();
  const app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir: path.join(dataDir, 'dl'), maxActiveDownloads: 1 }) });
    const first = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: t1.toString('base64') }) });
    const second = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: t2.toString('base64') }) });
    assert.notEqual(first.body.status, 'queued');
    assert.equal(second.body.status, 'queued');
    await wait(200);
    const byId = (l, id) => l.find((d) => d.id === id);
    let list = (await call(app, 'downloads')).body;
    assert.equal(byId(list, second.body.id).status, 'queued');
    assert.equal(byId(list, first.body.id).progress, 0);

    open();
    gate = Promise.resolve();
    list = await waitFor(app, (l) => l.every((d) => d.status === 'done'));
    assert.ok(byId(list, second.body.id).completedAt >= byId(list, first.body.id).completedAt);
    assert.ok(fs.readFileSync(path.join(dataDir, 'dl', 'iki.mp4')).equals(b));
  } finally {
    open();
    await app.close();
    await new Promise((r) => server.close(r));
  }
});

test('duraklatma sırayı boşaltır, sıradaki başlar', async () => {
  const dataDir = tmp();
  const app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir: path.join(dataDir, 'dl'), maxActiveDownloads: 1 }) });
    const one = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ magnet: `magnet:?xt=urn:btih:${'1'.repeat(40)}` }) });
    const two = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ magnet: `magnet:?xt=urn:btih:${'2'.repeat(40)}` }) });
    assert.equal(two.body.status, 'queued');
    await call(app, `downloads/${one.body.id}/pause`, { method: 'POST' });
    const list = (await call(app, 'downloads')).body;
    assert.equal(list.find((d) => d.id === two.body.id).status, 'connecting');
    // Kullanıcı "devam" derse sınırı beklemeden başlar.
    const resumed = await call(app, `downloads/${one.body.id}/resume`, { method: 'POST' });
    assert.equal(resumed.body.status, 'connecting');
  } finally {
    await app.close();
  }
});

test('hız sınırı ayarı motora uygulanır ve 0 kapatır', async () => {
  const calls = [];
  const api = {
    get: async () => [],
    post: async (p, body) => {
      if (p.startsWith('transfer/')) calls.push([p === 'transfer/setDownloadLimit' ? 'down' : 'up', Number(body.limit)]);
      return 'Ok.';
    },
  };
  const engine = createQbEngine({ start: async () => ({ api }) });
  engine.throttle({ download: 500 * 1024, upload: 0 });
  engine.add(`magnet:?xt=urn:btih:${'9'.repeat(40)}`, { path: tmp() }); // qBittorrent ilk eklemede açılır, sınır o an uygulanır
  await wait(50);
  assert.deepEqual(calls, [['down', 512000], ['up', 0]]);
  engine.throttle({ download: 0, upload: 64 * 1024 });
  await wait(20);
  assert.deepEqual(calls.slice(2), [['down', 0], ['up', 65536]]);
  await engine.destroy();
});

test('kuyruk ve hız ayarları sınırlanır, eski ayar dosyası varsayılanları alır', () => {
  const file = path.join(tmp(), 'settings.json');
  fs.writeFileSync(file, JSON.stringify({ downloadDir: '/eski' }));
  const store = createSettingsStore(file);
  assert.equal(store.get().maxActiveDownloads, 3);
  assert.equal(store.get().downloadLimitKbps, 0);
  store.update({ maxActiveDownloads: 50, downloadLimitKbps: -5, uploadLimitKbps: '120' });
  assert.equal(store.get().maxActiveDownloads, 10);
  assert.equal(store.get().downloadLimitKbps, 0);
  assert.equal(store.get().uploadLimitKbps, 120);
  store.update({ maxActiveDownloads: 0 });
  assert.equal(store.get().maxActiveDownloads, 1);
  assert.equal(store.get().downloadDir, '/eski');
});
