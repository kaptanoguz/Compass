import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { createQbEngine, launchQbittorrent, findQbittorrent } from '../src/qbittorrent.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-qb-'));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const hash = 'c12fe1c06bba254a9dc9f519b335aa7c1367a88a';

// Yalnızca çağrıları kaydeden sahte qBittorrent API'si.
function fakeApi() {
  const calls = [];
  const torrents = new Set();
  return {
    calls,
    api: {
      async get(p, params) {
        if (p === 'torrents/info') return [...torrents].filter((h) => !params?.hashes || params.hashes === h).map((h) => ({ hash: h, state: 'metaDL', total_size: 0 }));
        return [];
      },
      async post(p, body) {
        calls.push(p);
        if (p === 'torrents/add') torrents.add(hash);
        if (p === 'torrents/delete') torrents.delete(body.get ? body.get('hashes') : body.hashes);
        return 'Ok.';
      },
    },
  };
}

test('duraklatılan torrent qBittorrent\'te bekler, devam edince yeniden eklenmez (dosyalar baştan kontrol edilmez)', async () => {
  const { api, calls } = fakeApi();
  const engine = createQbEngine({ start: async () => ({ api }), pollMs: 20 });
  const magnet = `magnet:?xt=urn:btih:${hash}`;
  const a = engine.add(magnet, { path: tmp() });
  await wait(50);
  engine.pause(a);
  await wait(20);
  const b = engine.add(magnet, { path: tmp() });
  await wait(50);
  assert.equal(calls.filter((c) => c === 'torrents/add').length, 1, 'ikinci kez eklenmedi');
  assert.ok(!calls.includes('torrents/delete'), 'duraklatma silmez');
  engine.remove(b); // listeden kaldırma ise qBittorrent'ten de siler (dosyalar kalır)
  await wait(20);
  assert.ok(calls.includes('torrents/delete'));
  await engine.destroy();
});

test('çöken oturumdan kalan qBittorrent kapatılır, yenisi aynı profille açılır', { skip: !findQbittorrent() && 'qbittorrent-nox kurulu değil' }, async () => {
  const profileDir = tmp();
  await launchQbittorrent({ profileDir }); // stop çağrılmaz: uygulama çökmüş gibi
  const old = Number(fs.readFileSync(path.join(profileDir, 'torrent-bul.pid'), 'utf8'));
  const qb = await launchQbittorrent({ profileDir });
  try {
    assert.match(await qb.api.get('app/version'), /^v\d/);
    assert.throws(() => process.kill(old, 0), 'eski süreç kapandı');
  } finally {
    await qb.stop();
  }
});

test('Jackett bağlantısı magnet\'e yönlendirirse magnet kullanılır; 4xx kalıcı hata sayılır', async () => {
  const { fetchTorrentSource } = await import('../src/qbittorrent.js');
  const { isTransientError } = await import('../src/downloads.js');
  const http = await import('node:http');
  const magnet = `magnet:?xt=urn:btih:${hash}&dn=Film`;
  const server = http.createServer((req, res) => {
    if (req.url === '/dl/magnet') { res.writeHead(302, { Location: magnet }); return res.end(); }
    if (req.url === '/dl/zincir') { res.writeHead(301, { Location: '/dl/dosya' }); return res.end(); }
    if (req.url === '/dl/dosya') { res.writeHead(200); return res.end('d4:infod4:name1:xee'); }
    res.writeHead(404);
    res.end();
  });
  await new Promise((r) => server.listen(0, '127.0.0.1', r));
  const base = `http://127.0.0.1:${server.address().port}`;
  try {
    assert.deepEqual(await fetchTorrentSource(`${base}/dl/magnet`), { magnet });
    assert.equal((await fetchTorrentSource(`${base}/dl/zincir`)).buf.toString(), 'd4:infod4:name1:xee');
    const err = await fetchTorrentSource(`${base}/dl/yok`).catch((e) => e);
    assert.match(err.message, /HTTP 404/);
    assert.equal(isTransientError(err.message), false, '404 yeniden denenmez');
  } finally {
    server.close();
  }
});
