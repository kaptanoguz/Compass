import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';
import { createLibrary } from '../src/library.js';
import { addImdbRatings, createRatingsCache } from '../src/ratings.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-lib2-'));
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
  const video = Buffer.alloc(150 * 1024, 4);
  let torrentBuf;
  const server = http.createServer((req, res) => {
    if (req.url === '/item.torrent') { res.writeHead(200); return res.end(torrentBuf); }
    const m = /bytes=(\d+)-(\d+)?/.exec(req.headers.range || '');
    const start = m ? Number(m[1]) : 0;
    const end = m && m[2] ? Number(m[2]) : video.length - 1;
    res.writeHead(m ? 206 : 200, { 'Content-Length': end - start + 1, ...(m ? { 'Content-Range': `bytes ${start}-${end}/${video.length}` } : {}) });
    res.end(video.subarray(start, end + 1));
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  torrentBuf = Buffer.from(await new Promise((resolve, reject) =>
    createTorrent(Object.assign(Buffer.from(video), { name: 'nosferatu.mp4' }), { name: 'nosferatu.mp4', urlList: [`${base}/`], announceList: [] },
      (e, t) => (e ? reject(e) : resolve(t)))));
  return { server, url: `${base}/item.torrent` };
}

test('biten indirme kütüphaneye girer, ilerleme ikinci oturumda okunur, kayıt silinse de durur', async () => {
  const { server, url } = await fixture();
  const dataDir = tmp();
  const downloadDir = path.join(dataDir, 'dl');
  let app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir }) });
    await call(app, 'downloads', {
      method: 'POST',
      body: JSON.stringify({
        torrentUrl: url, title: 'Nosferatu', source: 'Internet Archive', year: 1922, genres: ['Korku'],
        poster: 'https://archive.org/services/img/nosferatu', pageUrl: 'https://archive.org/details/nosferatu',
        license: 'Kamu malı', imdbId: 'tt0013442', evil: '<script>',
      }),
    });
    const [done] = await waitFor(app, (l) => l[0]?.status === 'done');

    const [entry] = (await call(app, 'library')).body;
    assert.equal(entry.title, 'Nosferatu');
    assert.equal(entry.year, 1922);
    assert.equal(entry.poster, 'https://archive.org/services/img/nosferatu');
    assert.deepEqual(entry.genres, ['Korku']);
    assert.equal(entry.license, 'Kamu malı');
    assert.equal(entry.imdbId, 'tt0013442');
    assert.equal(entry.evil, undefined);
    assert.equal(entry.items.length, 1);
    assert.equal(entry.items[0].downloadId, done.id);
    assert.equal(entry.items[0].missing, false);
    assert.equal(entry.items[0].path, undefined, 'tam yol istemciye gönderilmez');

    const put = await call(app, `library/${entry.infoHash}/progress`, { method: 'PUT', body: JSON.stringify({ fileIndex: 0, positionSec: 61.4, durationSec: 5400 }) });
    assert.equal(put.status, 200);
    assert.equal(put.body.items[0].positionSec, 61);
    assert.equal((await call(app, 'library/yok/progress', { method: 'PUT', body: JSON.stringify({ fileIndex: 0 }) })).status, 404);

    // İndirme listeden kaldırılınca kütüphane kaydı durur ve dosya hâlâ oynatılabilir.
    await call(app, `downloads/${done.id}`, { method: 'DELETE' });
    await app.close();
    app = await startServer({ dataDir, openPath: () => {} });
    const [again] = (await call(app, 'library')).body;
    assert.equal(again.items[0].positionSec, 61);
    assert.equal(again.items[0].durationSec, 5400);
    const media = await fetch(new URL(`media/lib-${again.infoHash}/0?t=${app.token}`, app.url), { headers: { Range: 'bytes=0-9' } });
    assert.equal(media.status, 206);

    // Dosya taşınınca kayıt "bulunamadı" olur ama silinmez.
    fs.rmSync(path.join(downloadDir, 'nosferatu.mp4'));
    const [moved] = (await call(app, 'library')).body;
    assert.equal(moved.missing, true);
    assert.equal(moved.items[0].missing, true);
  } finally {
    await app.close();
    await new Promise((r) => server.close(r));
  }
});

test('izleme konumu diske en fazla aralıkla yazılır', async () => {
  const file = path.join(tmp(), 'library.json');
  const lib = createLibrary({ file, saveDelayMs: 200 });
  lib.upsert({ infoHash: 'a'.repeat(40), title: 'Film', downloadId: 'x', files: [{ index: 0, name: 'film.mp4', path: '/yok/film.mp4' }] });
  const read = () => JSON.parse(fs.readFileSync(file, 'utf8'))['a'.repeat(40)].items[0].positionSec;
  for (let s = 1; s <= 5; s++) lib.progress('a'.repeat(40), { fileIndex: 0, positionSec: s });
  assert.equal(read(), 0, 'hemen yazılmadı');
  await wait(300);
  assert.equal(read(), 5);
  lib.progress('a'.repeat(40), { fileIndex: 0, positionSec: 9, watched: true });
  lib.close();
  assert.equal(read(), 9);
});

test('OMDb önbelleği yeniden açılışta kullanılır, aynı ad ikinci kez sorulmaz', async () => {
  const file = path.join(tmp(), 'omdb-cache.json');
  let calls = 0;
  const fakeFetch = async (url) => {
    calls++;
    const t = new URL(url).searchParams.get('t');
    const body = t === 'Nosferatu'
      ? { Response: 'True', imdbID: 'tt0013442', imdbRating: '7.9', imdbVotes: '1,000', Genre: 'Horror', Year: '1922', Poster: 'N/A' }
      : { Response: 'False', Error: 'Movie not found!' };
    return new Response(JSON.stringify(body), { status: 200 });
  };
  const results = () => [
    { title: 'Nosferatu (1922)', category: 'movies' },
    { title: 'Bilinmeyen Yapim 2031', category: 'movies' },
  ];
  const first = await addImdbRatings(results(), 'k', { fetch: fakeFetch, ratingsCache: createRatingsCache(file) });
  assert.equal(first[0].rating.value, 7.9);
  assert.equal(calls, 2);

  // Yeni oturum: önbellek dosyadan okunur; bulunan da bulunamayan da tekrar sorulmaz.
  const second = await addImdbRatings(results(), 'k', { fetch: fakeFetch, ratingsCache: createRatingsCache(file) });
  assert.equal(second[0].rating.value, 7.9);
  assert.equal(second[0].imdbUrl, 'https://www.imdb.com/title/tt0013442/');
  assert.equal(calls, 2);

  // "Bulunamadı" 6 saat sonra yeniden denenir, bulunan 30 gün geçerli.
  const cache = createRatingsCache(file);
  const later = Date.now() + 7 * 3600e3;
  assert.equal(cache.get('bilinmeyen yapim|2031|false', later), undefined);
  assert.notEqual(cache.get('nosferatu|1922|false', later), undefined);
});

test('önbellekteki sonuçlar 20 sorgu kotasından düşmez', async () => {
  let calls = 0;
  const fakeFetch = async () => { calls++; return new Response(JSON.stringify({ Response: 'True', imdbID: 'tt0000001', imdbRating: '6.0', Year: '2000' }), { status: 200 }); };
  const cache = createRatingsCache(null);
  const list = (n, from = 0) => Array.from({ length: n }, (_, i) => ({ title: `Film ${from + i} (2000)`, category: 'movies' }));
  await addImdbRatings(list(20), 'k', { fetch: fakeFetch, ratingsCache: cache });
  assert.equal(calls, 20);
  const mixed = await addImdbRatings([...list(20), ...list(20, 100)], 'k', { fetch: fakeFetch, ratingsCache: cache });
  assert.equal(calls, 40);
  assert.ok(mixed.every((r) => r.rating));
});

test('OMDb cevabı arama bitmeden diske yazılır', async () => {
  const file = path.join(tmp(), 'omdb-cache.json');
  // İlk yapım hemen döner, ikincisi hiç dönmez (arama yarıda kalır).
  const fakeFetch = (url) => {
    if (new URL(url).searchParams.get('t') === 'Nosferatu') {
      return Promise.resolve(new Response(JSON.stringify({ Response: 'True', imdbID: 'tt0013442', imdbRating: '7.9', Year: '1922' }), { status: 200 }));
    }
    return new Promise(() => {});
  };
  const pending = addImdbRatings([
    { title: 'Nosferatu (1922)', category: 'movies' },
    { title: 'Asla Gelmeyen 1930', category: 'movies' },
  ], 'k', { fetch: fakeFetch, ratingsCache: createRatingsCache(file) });
  await wait(50);
  const saved = JSON.parse(fs.readFileSync(file, 'utf8'));
  assert.equal(saved['nosferatu|1922|false'].value.imdbId, 'tt0013442');
  // Yeni oturum bu yapımı tekrar sormaz.
  let calls = 0;
  await addImdbRatings([{ title: 'Nosferatu (1922)', category: 'movies' }], 'k', { fetch: async () => { calls++; return new Response('{}'); }, ratingsCache: createRatingsCache(file) });
  assert.equal(calls, 0);
  void pending;
});

test('OMDb anahtarı yoksa Cinemeta\'dan afiş ve IMDb puanı gelir; ad ve yıl eşleşmeli', async () => {
  const asked = [];
  const fakeFetch = async (url) => {
    const u = String(url);
    asked.push(u);
    if (u.includes('/catalog/series/top/search=Dopesick.json')) {
      return new Response(JSON.stringify({ metas: [
        { id: 'tt9999999', name: 'Dopesick Diaries', releaseInfo: '2019' },
        { id: 'tt11121604', name: 'Dopesick', releaseInfo: '2021', poster: 'https://images.metahub.space/poster/medium/tt11121604/img' },
      ] }));
    }
    if (u.endsWith('/meta/series/tt11121604.json')) {
      return new Response(JSON.stringify({ meta: { id: 'tt11121604', name: 'Dopesick', imdbRating: '8.5', genres: ['Drama'], releaseInfo: '2021', poster: 'https://images.metahub.space/poster/medium/tt11121604/img' } }));
    }
    if (u.includes('/catalog/movie/top/search=Dune%20Part%20Two.json')) {
      return new Response(JSON.stringify({ metas: [{ id: 'tt15239678', name: 'Dune: Part Two', releaseInfo: '2024', imdbRating: '8.6', poster: 'https://images.metahub.space/poster/medium/tt15239678/img' }] }));
    }
    return new Response(JSON.stringify({ metas: [] }));
  };
  const results = [
    { title: 'Dopesick S01 COMPLETE 1080p HULU 10bit DDP 5 1 x265 [HashMiner]', category: 'tv' },
    { title: 'Dune.Part.Two.2024.2160p.WEB-DL', category: 'movies' },
    { title: 'Bilinmeyen Yapim 2031', category: 'movies' },
  ];
  await addImdbRatings(results, '', { fetch: fakeFetch, ratingsCache: createRatingsCache(null) });
  assert.equal(results[0].rating.value, 8.5);
  assert.equal(results[0].imdbUrl, 'https://www.imdb.com/title/tt11121604/');
  assert.match(results[0].poster, /tt11121604/);
  assert.equal(results[1].rating.value, 8.6, 'iki nokta üst üste ve büyük harf farkı eşleşmeyi bozmaz');
  assert.equal(results[2].rating, undefined);
  assert.ok(asked.every((u) => u.startsWith('https://v3-cinemeta.strem.io/')));
});

test('yavaş bilgi servisi aramayı bekletmez; geç gelen cevap sonraki aramada hazırdır', async () => {
  let release;
  const gate = new Promise((r) => { release = r; });
  const fakeFetch = async () => {
    await gate;
    return new Response(JSON.stringify({ metas: [{ id: 'tt0013442', name: 'Nosferatu', releaseInfo: '1922', imdbRating: '7.9' }] }));
  };
  const cache = createRatingsCache(null);
  const started = Date.now();
  const first = await addImdbRatings([{ title: 'Nosferatu (1922)', category: 'movies' }], '', { fetch: fakeFetch, ratingsCache: cache, budgetMs: 100 });
  assert.ok(Date.now() - started < 1000);
  assert.equal(first[0].rating, undefined);
  release();
  await wait(50);
  const second = await addImdbRatings([{ title: 'Nosferatu (1922)', category: 'movies' }], '', { fetch: fakeFetch, ratingsCache: cache, budgetMs: 100 });
  assert.equal(second[0].rating.value, 7.9);
});
