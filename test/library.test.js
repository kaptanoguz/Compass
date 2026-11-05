import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';
import { subtitleToVtt } from '../src/downloads.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-lib-'));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function call(app, p, opts = {}) {
  const res = await fetch(new URL(`api/${p}`, app.url), {
    ...opts,
    headers: { 'X-App-Token': app.token, 'Content-Type': 'application/json' },
  });
  return { status: res.status, body: await res.json() };
}

async function waitFor(app, check) {
  let list;
  for (let i = 0; i < 100; i++) {
    list = (await call(app, 'downloads')).body;
    if (check(list)) return list;
    await wait(100);
  }
  // Son durum hatada görünsün: CI'da neden beklendiği ancak böyle anlaşılır.
  throw new Error(`zaman aşımı; son durum: ${JSON.stringify(list.map(({ status, error, missing, progress, files }) => ({ status, error, missing, progress, files: files?.length })))}`);
}

// "Ğüşİöç" Windows-1254 kodlamasıyla, Türkçe altyazılarda sık görülen hâli.
const SRT_1254 = Buffer.concat([
  Buffer.from('1\r\n00:00:01,000 --> 00:00:02,500\r\n'),
  Buffer.from([0xd0, 0xfc, 0xfe, 0xdd, 0xf6, 0xe7]),
  Buffer.from('\r\n'),
]);

test('SRT altyazı WebVTT’ye çevrilir, Windows-1254 Türkçe karakterler korunur', () => {
  const vtt = subtitleToVtt(SRT_1254, 'film.tr.srt');
  assert.match(vtt, /^WEBVTT\n\n1\n00:00:01\.000 --> 00:00:02\.500\nĞüşİöç\n$/);
});

test('biten indirme kapanır, video ve altyazı sunulur, taşınan dosya tekrar indirilmez', async () => {
  const video = Buffer.alloc(300 * 1024);
  for (let i = 0; i < video.length; i++) video[i] = (i * 7) % 253;
  const files = { 'film.mp4': video, 'film.tr.srt': SRT_1254 };

  let torrentBuf;
  const server = http.createServer((req, res) => {
    if (process.env.QB_DEBUG) console.error('[qb-debug] http', req.method, req.url, req.headers.range || '', req.headers['user-agent'] || '');
    if (req.url === '/item.torrent') { res.writeHead(200); return res.end(torrentBuf); }
    const name = decodeURIComponent(req.url).replace(/^\/Film\//, '');
    const body = files[name];
    if (!body) { res.writeHead(404); return res.end(); }
    const m = /bytes=(\d+)-(\d+)?/.exec(req.headers.range || '');
    const start = m ? Number(m[1]) : 0;
    const end = m && m[2] ? Number(m[2]) : body.length - 1;
    res.writeHead(m ? 206 : 200, { 'Content-Length': end - start + 1, ...(m ? { 'Content-Range': `bytes ${start}-${end}/${body.length}` } : {}) });
    res.end(body.subarray(start, end + 1));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  const input = Object.entries(files).map(([name, buf]) => Object.assign(Buffer.from(buf), { name }));
  torrentBuf = await new Promise((resolve, reject) =>
    createTorrent(input, { name: 'Film', urlList: [`${base}/`], announceList: [] }, (err, t) => (err ? reject(err) : resolve(t))));

  const dataDir = tmp();
  const downloadDir = path.join(dataDir, 'indirilenler');
  let app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir }) });
    const added = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: `${base}/item.torrent`, title: 'Film' }) });
    const id = added.body.id;
    // İki dosyalı torrent: metadata gelince indirme seçimi bekler.
    await waitFor(app, (l) => l[0]?.status === 'picking');
    assert.equal((await call(app, `downloads/${id}/files`, { method: 'POST', body: JSON.stringify({ indexes: [0, 1] }) })).status, 200);
    const [done] = await waitFor(app, (l) => l[0]?.status === 'done');
    assert.ok(done.completedAt);
    assert.equal(done.missing, false);
    assert.deepEqual(done.videos.map((v) => v.name), ['film.mp4']);
    const index = done.videos[0].index;

    // Video, oynatıcının istediği aralıklarla sunulur; anahtarsız istek reddedilir.
    const media = new URL(`media/${id}/${index}?t=${app.token}`, app.url);
    const part = await fetch(media, { headers: { Range: 'bytes=100-199' } });
    assert.equal(part.status, 206);
    assert.equal(part.headers.get('content-type'), 'video/mp4');
    assert.ok(Buffer.from(await part.arrayBuffer()).equals(video.subarray(100, 200)));
    assert.equal((await fetch(new URL(`media/${id}/${index}?t=yanlis`, app.url))).status, 403);
    const vtt = await (await fetch(new URL(`media/${id}/${index}/subtitle.vtt?t=${app.token}`, app.url))).text();
    assert.match(vtt, /Ğüşİöç/);

    // Kullanıcı dosyaları başka yere taşır.
    fs.renameSync(path.join(downloadDir, 'Film'), path.join(dataDir, 'arsiv'));
    const [moved] = (await call(app, 'downloads')).body;
    assert.equal(moved.status, 'done');
    assert.equal(moved.missing, true);
    assert.deepEqual(moved.videos, []);

    // Uygulama yeniden açılınca tekrar indirmeye başlamaz.
    await app.close();
    app = await startServer({ dataDir, openPath: () => {} });
    await wait(1500);
    const [afterRestart] = (await call(app, 'downloads')).body;
    assert.equal(afterRestart.status, 'done');
    assert.equal(afterRestart.missing, true);
    assert.ok(!fs.existsSync(path.join(downloadDir, 'Film')), 'taşınan dosyalar yeniden indirilmedi');

    // Kullanıcı aynı sonuçta "Tekrar indir"e basarsa bilerek yeniden indirilir.
    await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ torrentUrl: `${base}/item.torrent` }) });
    await waitFor(app, (l) => l[0]?.status === 'done' && !l[0].missing);
    assert.ok(fs.readFileSync(path.join(downloadDir, 'Film', 'film.mp4')).equals(video));
  } finally {
    await app.close();
    await new Promise((r) => server.close(r));
  }
});
