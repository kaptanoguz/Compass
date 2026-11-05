import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-imp-'));
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

async function fixture() {
  const content = Buffer.alloc(200 * 1024, 9);
  const server = http.createServer((req, res) => {
    const m = /bytes=(\d+)-(\d+)?/.exec(req.headers.range || '');
    const start = m ? Number(m[1]) : 0;
    const end = m && m[2] ? Number(m[2]) : content.length - 1;
    res.writeHead(m ? 206 : 200, { 'Content-Length': end - start + 1, ...(m ? { 'Content-Range': `bytes ${start}-${end}/${content.length}` } : {}) });
    res.end(content.subarray(start, end + 1));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const url = `http://127.0.0.1:${server.address().port}/dizi.mkv`;
  const make = (name, priv) => new Promise((resolve, reject) =>
    createTorrent(Object.assign(Buffer.from(content), { name }), { name, urlList: [url], announceList: [], private: priv }, (e, t) => (e ? reject(e) : resolve(t))));
  return { content, server, a: Buffer.from(await make('dizi.mkv', true)), b: Buffer.from(await make('belgesel.mkv', false)) };
}

test('bilgisayardaki .torrent dosyaları incelenir ve toplu eklenir', async () => {
  const { server, a, b } = await fixture();
  const dataDir = tmp();
  const app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir: path.join(dataDir, 'dl') }) });
    const files = [
      { name: 'dizi.torrent', data: a.toString('base64') },
      { name: 'belgesel.torrent', data: b.toString('base64') },
      { name: 'bozuk.torrent', data: Buffer.from('merhaba').toString('base64') },
    ];
    const { body: infos } = await call(app, 'torrents/inspect', { method: 'POST', body: JSON.stringify({ files }) });
    assert.equal(infos[0].name, 'dizi.mkv');
    assert.equal(infos[0].private, true);
    assert.equal(infos[0].length, 200 * 1024);
    assert.equal(infos[1].private, false);
    assert.equal(infos[2].error, 'Geçerli bir .torrent dosyası değil.');

    const first = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: files[0].data }) });
    assert.equal(first.status, 201);
    assert.equal(first.body.title, 'dizi.mkv');
    const again = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: files[0].data }) });
    assert.equal(again.body.id, first.body.id, 'aynı torrent iki kez eklenmez');
    await waitFor(app, (l) => l.length === 1 && l[0].status === 'done');
    const { body: [seen] } = await call(app, 'torrents/inspect', { method: 'POST', body: JSON.stringify({ files: [files[0]] }) });
    assert.equal(seen.existing, 'done');
  } finally {
    await app.close();
    await new Promise((r) => server.close(r));
  }
});

test('paylaşım açıkken biten torrent paylaşılır; dosya taşınınca durur ve yeniden inmez', async () => {
  const { server, a } = await fixture();
  const dataDir = tmp();
  const dl = path.join(dataDir, 'dl');
  let app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir: dl, seedAfterDone: true }) });
    await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentFile: a.toString('base64') }) });
    let [d] = await waitFor(app, (l) => l[0]?.status === 'done');
    await wait(300);
    [d] = (await call(app, 'downloads')).body;
    assert.equal(d.seeding, true);

    // Yeniden açılışta da paylaşım sürer (dosyalar yerindeyken).
    await app.close();
    app = await startServer({ dataDir, openPath: () => {} });
    await wait(500);
    [d] = (await call(app, 'downloads')).body;
    assert.equal(d.status, 'done');
    assert.equal(d.seeding, true);

    // Paylaşımı kapatmak torrenti kapatır.
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ seedAfterDone: false }) });
    [d] = (await call(app, 'downloads')).body;
    assert.equal(d.seeding, false);
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ seedAfterDone: true }) });
    await wait(300);
    [d] = (await call(app, 'downloads')).body;
    assert.equal(d.seeding, true);

    // Dosya taşınınca paylaşım durur, indirme başlamaz.
    fs.renameSync(path.join(dl, 'dizi.mkv'), path.join(dataDir, 'tasindi.mkv'));
    [d] = (await call(app, 'downloads')).body;
    assert.equal(d.seeding, false);
    assert.equal(d.missing, true);
    await app.close();
    app = await startServer({ dataDir, openPath: () => {} });
    await wait(1500);
    [d] = (await call(app, 'downloads')).body;
    assert.equal(d.status, 'done');
    assert.equal(d.seeding, false);
    assert.ok(!fs.existsSync(path.join(dl, 'dizi.mkv')), 'taşınan dosya yeniden indirilmedi');
  } finally {
    await app.close();
    await new Promise((r) => server.close(r));
  }
});
