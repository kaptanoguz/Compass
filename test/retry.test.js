import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { EventEmitter } from 'node:events';
import { createDownloadManager, isTransientError, MAX_RETRIES, BLOCKED_MESSAGE } from '../src/downloads.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-retry-'));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));
const magnet = (c) => `magnet:?xt=urn:btih:${c.repeat(40)}`;

// qBittorrent yerine geçen sahte motor: eklenen her torrenti kaydeder.
function fakeEngine() {
  const added = [];
  return {
    added,
    add(source) {
      const t = Object.assign(new EventEmitter(), { source, at: Date.now(), destroyed: false, files: [], pieces: [], progress: 0, downloaded: 0, length: 0, uploaded: 0 });
      added.push(t);
      return t;
    },
    pause(t) { if (t) t.destroyed = true; },
    selectFiles() {}, deselectAll() {}, selectAll() {}, throttle() {},
    destroy: () => Promise.resolve(),
  };
}

function manager(engine, opts = {}) {
  const dir = opts.dir || tmp();
  return createDownloadManager({ stateFile: path.join(dir, 'downloads.json'), getDownloadDir: () => path.join(dir, 'dl'), engine, retryBaseMs: 10, ...opts });
}

test('geçici ağ hataları ayırt edilir', () => {
  for (const m of ['fetch failed', 'getaddrinfo ENOTFOUND archive.org', 'connect ECONNREFUSED 127.0.0.1:80', 'read ECONNRESET', 'Error downloading torrent: socket hang up', 'Request timed out']) {
    assert.equal(isTransientError(m), true, m);
  }
  for (const m of ['Invalid torrent identifier', 'Torrent is missing required field: info', 'ENOSPC: no space left on device', BLOCKED_MESSAGE]) {
    assert.equal(isTransientError(m), false, m);
  }
});

test('geçici hata artan aralıkla en fazla 5 kez yeniden denenir, sonra hata olur', async () => {
  const engine = fakeEngine();
  const m = manager(engine);
  try {
    const d = await m.add({ magnet: magnet('a') });
    const times = [];
    for (let i = 0; i <= MAX_RETRIES; i++) {
      const before = engine.added.length;
      times.push(Date.now()); // zamanlayıcı kurulmadan önce
      engine.added.at(-1).emit('error', new Error('connect ETIMEDOUT 1.2.3.4:6881'));
      if (i < MAX_RETRIES) {
        assert.equal(m.get(d.id).status, 'connecting');
        assert.equal(m.get(d.id).retry, i + 1);
        assert.equal(m.get(d.id).error, null);
        for (let k = 0; k < 600 && engine.added.length === before; k++) await wait(5);
        assert.equal(engine.added.length, before + 1, 'yeniden eklendi');
        assert.ok(engine.added.at(-1).at - times[i] >= 10 * 2 ** i - 1, 'gecikme her seferinde ikiye katlanır');
      }
    }
    const final = m.get(d.id);
    assert.equal(final.status, 'error');
    assert.match(final.error, /zaman aşımı|Bağlantı/);
    assert.equal(engine.added.length, MAX_RETRIES + 1);
  } finally {
    await m.close();
  }
});

test('kalıcı hata yeniden denenmez; "Tekrar dene" sırayı beklemeden başlatır', async () => {
  const engine = fakeEngine();
  const m = manager(engine, { maxActive: () => 1 });
  try {
    const bad = await m.add({ magnet: magnet('b') });
    engine.added.at(-1).emit('error', new Error('Invalid torrent identifier'));
    await wait(40);
    assert.equal(m.get(bad.id).status, 'error');
    assert.equal(engine.added.length, 1);

    await m.add({ magnet: magnet('c') }); // tek yeri kaplar
    assert.equal(engine.added.length, 2);
    const again = m.resume(bad.id);
    assert.equal(again.status, 'connecting');
    assert.equal(engine.added.length, 3);
  } finally {
    await m.close();
  }
});

test('beklerken duraklatılan ya da silinen indirme yeniden denenmez', async () => {
  const engine = fakeEngine();
  const m = manager(engine);
  try {
    const a = await m.add({ magnet: magnet('d') });
    const b = await m.add({ magnet: magnet('e') });
    engine.added[0].emit('error', new Error('fetch failed'));
    engine.added[1].emit('error', new Error('fetch failed'));
    m.pause(a.id);
    m.remove(b.id);
    await wait(50);
    assert.equal(engine.added.length, 2);
    assert.equal(m.get(a.id).status, 'paused');
  } finally {
    await m.close();
  }
});

test('açılışta hatalı indirmeler yeniden denenir; duraklatılan, biten, seçim bekleyen ve .exe engeli atlanır', async () => {
  const dir = tmp();
  const base = { path: path.join(dir, 'dl'), addedAt: 1, torrentFile: null, torrentUrl: null };
  fs.writeFileSync(path.join(dir, 'downloads.json'), JSON.stringify([
    { ...base, id: 'hata', magnet: magnet('1'), status: 'error', error: 'Torrent dosyası alınamadı.', retryCount: 5 },
    { ...base, id: 'engel', magnet: magnet('2'), status: 'error', error: BLOCKED_MESSAGE },
    { ...base, id: 'dur', magnet: magnet('3'), status: 'paused' },
    { ...base, id: 'bitti', magnet: magnet('4'), status: 'done', progress: 1 },
    { ...base, id: 'secim', magnet: magnet('5'), status: 'picking' },
  ]));
  const engine = fakeEngine();
  const m = manager(engine, { dir });
  try {
    assert.deepEqual(engine.added.map((t) => t.source), [magnet('1')]);
    assert.equal(m.get('hata').status, 'connecting');
    assert.equal(m.get('hata').retry, 0, 'deneme sayısı sıfırlandı');
    assert.equal(m.get('engel').status, 'error');
    assert.equal(m.get('dur').status, 'paused');
    assert.equal(m.get('bitti').status, 'done');
    assert.equal(m.get('secim').status, 'picking');
  } finally {
    await m.close();
  }
});
