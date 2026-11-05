import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { writeJsonAtomic, readJsonSafe } from '../src/jsonfile.js';
import { createSettingsStore, DEFAULTS } from '../src/settings.js';
import { createDownloadManager, magnetInfoHash, playableInBrowser } from '../src/downloads.js';
import { searchAll } from '../src/search.js';
import archive from '../src/providers/archive.js';
import { createTorznabProvider } from '../src/providers/torznab.js';
import { startServer } from '../src/server.js';
import { createQbEngine } from '../src/qbittorrent.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-state-'));

function base32(hex) {
  const bits = [...hex].map((c) => parseInt(c, 16).toString(2).padStart(4, '0')).join('');
  return bits.match(/.{5}/g).map((b) => 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'[parseInt(b, 2)]).join('');
}

test('atomik yazma: taşıma başarısız olursa eski dosya bozulmaz, yarım dosya kalmaz', (t) => {
  const dir = tmp();
  const file = path.join(dir, 'downloads.json');
  writeJsonAtomic(file, [{ id: 'eski' }]);
  t.mock.method(fs, 'renameSync', () => { throw new Error('disk dolu'); });
  assert.throws(() => writeJsonAtomic(file, [{ id: 'yeni' }]), /disk dolu/);
  t.mock.restoreAll();
  assert.deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')), [{ id: 'eski' }]);
  assert.deepEqual(fs.readdirSync(dir).filter((f) => f.endsWith('.tmp')), []);
});

test('bozuk durum dosyası yedekten okunur, bozuk dosya silinmez', () => {
  const dir = tmp();
  const file = path.join(dir, 'downloads.json');
  writeJsonAtomic(file, [{ id: 'a' }]);
  writeJsonAtomic(file, [{ id: 'a' }, { id: 'b' }]);
  fs.writeFileSync(file, '[{"id": "a"}, {"id"'); // yarıda kesilmiş yazım
  const warn = console.warn;
  console.warn = () => {};
  try {
    assert.deepEqual(readJsonSafe(file, []), [{ id: 'a' }]);
  } finally {
    console.warn = warn;
  }
  assert.ok(fs.readdirSync(dir).some((f) => f.startsWith('downloads.json.bozuk-')));
  assert.deepEqual(readJsonSafe(path.join(dir, 'yok.json'), []), []);
});

test('eski settings.json varsayılanlarla birleşir', () => {
  const dir = tmp();
  const file = path.join(dir, 'settings.json');
  fs.writeFileSync(file, JSON.stringify({ downloadDir: '/eski/klasor', omdbApiKey: 'k' }));
  const s = createSettingsStore(file).get();
  assert.equal(s.downloadDir, '/eski/klasor');
  assert.equal(s.omdbApiKey, 'k');
  assert.equal(s.archiveEnabled, DEFAULTS.archiveEnabled);
  assert.equal(s.seedAfterDone, false);
});

test('magnet btih çözümlenir: hex ve base32 aynı değeri verir', () => {
  const hex = 'c12fe1c06bba254a9dc9f519b335aa7c1367a88a';
  assert.equal(magnetInfoHash(`magnet:?xt=urn:btih:${hex.toUpperCase()}&dn=x`), hex);
  assert.equal(magnetInfoHash(`magnet:?dn=y&xt=urn:btih:${base32(hex)}`), hex);
  assert.equal(magnetInfoHash('magnet:?dn=hash-yok'), null);
});

test('aynı btih ikinci kez eklenmez, var olan kayıt döner', async () => {
  const dir = tmp();
  const manager = createDownloadManager({ stateFile: path.join(dir, 'downloads.json'), getDownloadDir: () => path.join(dir, 'dl'), engine: createQbEngine({ profileDir: path.join(dir, 'qb') }) });
  try {
    const hex = 'c12fe1c06bba254a9dc9f519b335aa7c1367a88a';
    const a = await manager.add({ magnet: `magnet:?xt=urn:btih:${hex}&dn=Film`, title: 'Film' });
    const b = await manager.add({ magnet: `magnet:?xt=urn:btih:${base32(hex)}&dn=Baska+Ad&tr=udp%3A%2F%2Fx` });
    assert.equal(b.id, a.id);
    assert.equal(manager.list().length, 1);
    const saved = JSON.parse(fs.readFileSync(path.join(dir, 'downloads.json'), 'utf8'));
    assert.equal(saved.length, 1);
    assert.equal(saved[0].infoHash, hex);
  } finally {
    await manager.close();
  }
});

test('indirme listesini okumak diske yazmaz', async () => {
  const dataDir = tmp();
  const app = await startServer({ dataDir });
  try {
    const headers = { 'X-App-Token': app.token, 'Content-Type': 'application/json' };
    await fetch(new URL('api/downloads', app.url), {
      method: 'POST', headers, body: JSON.stringify({ magnet: 'magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a' }),
    });
    const file = path.join(dataDir, 'downloads.json');
    const before = fs.statSync(file).mtimeMs;
    await new Promise((r) => setTimeout(r, 30));
    for (let i = 0; i < 3; i++) assert.equal((await fetch(new URL('api/downloads', app.url), { headers })).status, 200);
    assert.equal(fs.statSync(file).mtimeMs, before);
  } finally {
    await app.close();
  }
});

test('yavaş kaynak zaman aşımına düşer, diğer kaynak sonuç döner', async () => {
  const xml = `<rss><channel><item><title>Hızlı Film 2020</title>
    <link>magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a</link>
    <torznab:attr name="seeders" value="5"/></item></channel></rss>`;
  // İstek iptal sinyaline uyan sahte fetch: Archive hiç yanıt vermez, Torznab hemen döner.
  const fakeFetch = (url, { signal } = {}) => {
    if (String(url).includes('archive.org')) {
      return new Promise((_, reject) => signal.addEventListener('abort', () => reject(signal.reason)));
    }
    return Promise.resolve(new Response(xml, { status: 200 }));
  };
  const jackett = createTorznabProvider({ id: 't', name: 'Jackett', url: 'http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab' });
  // AbortSignal.timeout zamanlayıcısı süreci canlı tutmaz; uygulamada sunucu tutar, testte biz tutalım.
  const keepAlive = setInterval(() => {}, 1000);
  const started = Date.now();
  const { results, errors } = await searchAll({ query: 'film' }, [archive, jackett], { fetch: fakeFetch, timeoutMs: 100 })
    .finally(() => clearInterval(keepAlive));
  assert.ok(Date.now() - started < 3000);
  assert.ok(results.some((r) => r.title === 'Hızlı Film 2020'));
  assert.deepEqual(errors, [{ provider: 'Internet Archive', message: 'Yanıt vermedi (zaman aşımı).' }]);
});

test('mkv ve hevc tarayıcıda oynatılamaz sayılır', () => {
  assert.equal(playableInBrowser('film.mkv'), false);
  assert.equal(playableInBrowser('film.avi'), false);
  assert.equal(playableInBrowser('Film.2020.x265.mp4'), false);
  assert.equal(playableInBrowser('Film.HEVC.mp4'), false);
  assert.equal(playableInBrowser('film.mp4'), true);
  assert.equal(playableInBrowser('film.webm'), true);
});
