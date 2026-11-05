import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-'));

async function call(app, p, opts = {}) {
  const res = await fetch(new URL(`api/${p}`, app.url), {
    ...opts,
    headers: { 'X-App-Token': app.token, 'Content-Type': 'application/json', ...(opts.headers || {}) },
  });
  return { status: res.status, body: await res.json() };
}

test('API anahtarsız istekleri reddeder, sayfaya anahtarı yerleştirir', async () => {
  const app = await startServer({ dataDir: tmp() });
  try {
    const res = await fetch(new URL('api/downloads', app.url));
    assert.equal(res.status, 403);
    const html = await (await fetch(app.url)).text();
    assert.ok(html.includes(app.token));
    const traversal = await fetch(new URL('/..%2fpackage.json', app.url));
    assert.notEqual(traversal.status, 200);
  } finally {
    await app.close();
  }
});

test('ayarlar kaydedilir ve API anahtarı gizlenir', async () => {
  const dataDir = tmp();
  const app = await startServer({ dataDir });
  try {
    const put = await call(app, 'settings', {
      method: 'PUT',
      body: JSON.stringify({ downloadDir: '/tmp/x', torznab: [{ name: 'J', url: 'http://localhost:9117/torznab', apiKey: 'secret' }] }),
    });
    assert.equal(put.body.torznab[0].apiKey, '••••••');
    // maskeli anahtarla tekrar kaydetmek gerçek anahtarı korur
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ torznab: put.body.torznab }) });
    const saved = JSON.parse(fs.readFileSync(path.join(dataDir, 'settings.json'), 'utf8'));
    assert.equal(saved.torznab[0].apiKey, 'secret');
    const meta = await call(app, 'meta');
    assert.deepEqual(meta.body.providers.map((p) => p.name), ['Internet Archive', 'J']);
  } finally {
    await app.close();
  }
});

test('web-seed içeren .torrent uçtan uca indirilir (Internet Archive gibi)', async () => {
  // Yerel bir "Internet Archive": dosyayı ve .torrent'i HTTP ile sunar.
  const content = Buffer.alloc(512 * 1024 + 123);
  for (let i = 0; i < content.length; i++) content[i] = (i * 31) % 251;
  const file = Object.assign(Buffer.from(content), { name: 'ornek-film.bin' });

  const files = http.createServer((req, res) => {
    if (req.url === '/item.torrent') { res.writeHead(200); return res.end(torrentBuf); }
    if (req.url === '/ornek-film.bin') {
      const m = /bytes=(\d+)-(\d+)?/.exec(req.headers.range || '');
      const start = m ? Number(m[1]) : 0;
      const end = m && m[2] ? Number(m[2]) : content.length - 1;
      res.writeHead(m ? 206 : 200, { 'Content-Length': end - start + 1, ...(m ? { 'Content-Range': `bytes ${start}-${end}/${content.length}` } : {}), 'Accept-Ranges': 'bytes' });
      return res.end(content.subarray(start, end + 1));
    }
    res.writeHead(404); res.end();
  });
  await new Promise((r) => files.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${files.address().port}`;
  const torrentBuf = await new Promise((resolve, reject) =>
    createTorrent(file, { name: 'ornek-film.bin', urlList: [`${base}/ornek-film.bin`], announceList: [] },
      (err, t) => (err ? reject(err) : resolve(t))));

  const dataDir = tmp();
  const downloadDir = path.join(dataDir, 'indirilenler');
  const opened = [];
  const app = await startServer({
    dataDir,
    openPath: (p) => opened.push(p),
  });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir }) });
    const added = await call(app, 'downloads', {
      method: 'POST',
      body: JSON.stringify({ torrentUrl: `${base}/item.torrent`, title: 'Örnek Film', source: 'Internet Archive' }),
    });
    assert.equal(added.status, 201);
    const again = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: `${base}/item.torrent` }) });
    assert.equal(again.body.id, added.body.id, 'aynı torrent iki kez eklenmez');

    let snap;
    for (let i = 0; i < 100; i++) {
      snap = (await call(app, 'downloads')).body[0];
      if (snap.status === 'done') break;
      await new Promise((r) => setTimeout(r, 100));
    }
    assert.equal(snap.status, 'done', JSON.stringify(snap));
    assert.equal(snap.progress, 1);
    assert.ok(fs.readFileSync(path.join(downloadDir, 'ornek-film.bin')).equals(content));

    const open = await call(app, `downloads/${snap.id}/open`, { method: 'POST' });
    assert.equal(open.status, 200);
    assert.equal(opened[0], path.join(downloadDir, 'ornek-film.bin'));

    const del = await call(app, `downloads/${snap.id}`, { method: 'DELETE' });
    assert.equal(del.status, 200);
    assert.equal((await call(app, 'downloads')).body.length, 0);
    assert.ok(fs.existsSync(path.join(downloadDir, 'ornek-film.bin')), 'listeden kaldırmak dosyayı silmez');
  } finally {
    await app.close();
    await new Promise((r) => files.close(r));
  }
});

test('geçersiz magnet reddedilir', async () => {
  const app = await startServer({ dataDir: tmp() });
  try {
    const res = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ magnet: 'javascript:alert(1)' }) });
    assert.equal(res.status, 400);
  } finally {
    await app.close();
  }
});

test('aramada yetişmeyen afiş ve puan sonradan istenebilir; yalnızca film ve dizi sorulur', async () => {
  const asked = [];
  const fakeFetch = async (url) => {
    const u = String(url);
    asked.push(u);
    if (u.includes('/catalog/movie/top/search=Dune.json')) {
      return new Response(JSON.stringify({ metas: [{ id: 'tt1160419', name: 'Dune', releaseInfo: '2021', poster: 'https://m.media-amazon.com/dune.jpg' }] }));
    }
    if (u.endsWith('/meta/movie/tt1160419.json')) {
      return new Response(JSON.stringify({ meta: { id: 'tt1160419', name: 'Dune', imdbRating: '8.0', genres: ['Drama'], releaseInfo: '2021', poster: 'https://images.metahub.space/poster/small/tt1160419/img' } }));
    }
    return new Response(JSON.stringify({ metas: [] }));
  };
  const app = await startServer({ dataDir: tmp(), fetchImpl: fakeFetch });
  try {
    const { status, body } = await call(app, 'ratings', { method: 'POST', body: JSON.stringify({ items: [
      { id: 'a', title: 'Dune: Çöl Gezegeni 2021 1080p TR Dublaj', year: 2021, category: 'movies' },
      { id: 'b', title: 'Bir Albüm 2021', year: 2021, category: 'audio' },
    ] }) });
    assert.equal(status, 200);
    assert.equal(body.ratings.a.rating.value, 8);
    assert.equal(body.ratings.a.imdbUrl, 'https://www.imdb.com/title/tt1160419/');
    assert.match(body.ratings.a.poster, /tt1160419/);
    assert.deepEqual(body.ratings.a.genres, ['dram']);
    assert.equal(body.ratings.b, undefined);
    assert.ok(asked.every((u) => !u.includes('Alb')), 'müzik sorulmaz');
  } finally {
    await app.close();
  }
});
