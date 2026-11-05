import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import http from 'node:http';
import createTorrent from 'create-torrent';
import { startServer } from '../src/server.js';
import { startSeeder } from './helpers/seeder.js';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'compass-player-'));
const wait = (ms) => new Promise((r) => setTimeout(r, ms));

async function call(app, p, opts = {}) {
  const res = await fetch(new URL(`api/${p}`, app.url), { ...opts, headers: { 'X-App-Token': app.token, 'Content-Type': 'application/json' } });
  return { status: res.status, body: await res.json() };
}

test('indirme bitmeden altyazı VTT olarak döner', async () => {
  const video = Buffer.alloc(256 * 1024, 6);
  const srt = Buffer.from('1\r\n00:00:01,000 --> 00:00:02,500\r\nMerhaba dünya\r\n');
  // Gerçek bir eş videoyu çok yavaş verir (saniyede ~20 KB); altyazı önce istendiği için hemen gelir.
  const seeder = await startSeeder({ 'film.mp4': video, 'film.srt': srt }, { name: 'Film', uploadLimit: 20 * 1024 });

  const dataDir = tmp();
  const app = await startServer({ dataDir, openPath: () => {} });
  try {
    await call(app, 'settings', { method: 'PUT', body: JSON.stringify({ downloadDir: path.join(dataDir, 'dl') }) });
    const { body: d } = await call(app, 'downloads', { method: 'POST', body: JSON.stringify({ magnet: seeder.magnet, fileIndexes: [0, 1] }) });
    // Dosya listesi motordan gelene kadar bekle (qBittorrent ayrı süreçte açılır).
    for (let i = 0; i < 100 && !(await call(app, 'downloads')).body[0].fileCount; i++) await wait(100);
    const res = await fetch(new URL(`media/${d.id}/0/subtitle.vtt?t=${app.token}`, app.url));
    assert.equal(res.status, 200);
    assert.match(await res.text(), /^WEBVTT\n\n1\n00:00:01\.000 --> 00:00:02\.500\nMerhaba dünya/);
    assert.notEqual((await call(app, 'downloads')).body[0].status, 'done');
  } finally {
    await app.close();
    await seeder.stop();
  }
});

test('yerel altyazı önizlemesi VTT döner, 1 MB üstü reddedilir, diske yazılmaz', async () => {
  const dataDir = tmp();
  const app = await startServer({ dataDir, openPath: () => {} });
  try {
    // Windows-1254 kodlu Türkçe altyazı.
    const srt = Buffer.concat([Buffer.from('1\r\n00:00:01,000 --> 00:00:02,000\r\n'), Buffer.from([0xd0, 0xfc, 0xfe])]);
    const ok = await call(app, 'subtitles/preview', { method: 'POST', body: JSON.stringify({ name: 'film.tr.srt', data: srt.toString('base64') }) });
    assert.equal(ok.status, 200);
    assert.match(ok.body.vtt, /^WEBVTT[\s\S]*00:00:01\.000 --> 00:00:02\.000\nĞüş/);
    const big = await call(app, 'subtitles/preview', { method: 'POST', body: JSON.stringify({ name: 'buyuk.srt', data: Buffer.alloc(1024 * 1024 + 1, 65).toString('base64') }) });
    assert.equal(big.status, 400);
    const bad = await call(app, 'subtitles/preview', { method: 'POST', body: JSON.stringify({ name: 'virus.exe', data: 'QQ==' }) });
    assert.equal(bad.status, 400);
    assert.deepEqual(fs.readdirSync(dataDir).filter((f) => /srt|vtt/.test(f)), []);
    const anon = await fetch(new URL('api/subtitles/preview', app.url), { method: 'POST', body: '{}' });
    assert.equal(anon.status, 403);
  } finally {
    await app.close();
  }
});
