import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { magnetFromArgv } from '../src/magnet.js';
import { createSettingsStore } from '../src/settings.js';

const HASH = 'c12fe1c06bba254a9dc9f519b335aa7c1367a88a';

test('komut satırından gelen magnet bulunur', () => {
  // Windows: "Compass.exe" --bayrak "magnet:?..."; Linux: AppImage yolu ve argümanlar.
  assert.equal(magnetFromArgv(['C:\\Program Files\\Compass\\Compass.exe', '--allow-file-access', `magnet:?xt=urn:btih:${HASH}&dn=Film`]), `magnet:?xt=urn:btih:${HASH}&dn=Film`);
  assert.equal(magnetFromArgv(['/opt/compass', `"magnet:?xt=urn:btih:${HASH}"`]), `magnet:?xt=urn:btih:${HASH}`);
  assert.equal(magnetFromArgv(['electron', '.']), null);
  assert.equal(magnetFromArgv(['magnet:?dn=hash-yok']), null);
  assert.equal(magnetFromArgv(['https://ornek.com/magnet:?xt=urn:btih:' + HASH]), null);
  assert.equal(magnetFromArgv([42, null]), null);
});

test('tepsiye küçültme isteğe bağlı ve varsayılan kapalı', () => {
  const file = path.join(fs.mkdtempSync(path.join(os.tmpdir(), 'compass-rel-')), 'settings.json');
  fs.writeFileSync(file, JSON.stringify({ downloadDir: '/eski' }));
  const store = createSettingsStore(file);
  assert.equal(store.get().closeToTray, false);
  store.update({ closeToTray: true });
  assert.equal(store.get().closeToTray, true);
  store.update({ closeToTray: 'evet' });
  assert.equal(store.get().closeToTray, true);
});

test('yayın dosyaları yerinde: lisans, CI iş akışı, ikon', () => {
  const root = path.join(path.dirname(new URL(import.meta.url).pathname), '..');
  assert.match(fs.readFileSync(path.join(root, 'LICENSE'), 'utf8'), /^MIT License\n\nCopyright \(c\) \d{4} kaptanoguz/);
  const pkg = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));
  assert.equal(pkg.license, 'MIT');
  assert.ok(fs.existsSync(path.join(root, pkg.build.icon)));
  const png = fs.readFileSync(path.join(root, pkg.build.icon));
  assert.equal(png.readUInt32BE(16), 512); // genişlik
  assert.equal(png.readUInt32BE(20), 512); // yükseklik
  const wf = fs.readFileSync(path.join(root, '.github/workflows/test.yml'), 'utf8');
  assert.match(wf, /node-version: 20/);
  assert.match(wf, /npm test/);
  assert.match(wf, /ELECTRON_SKIP_BINARY_DOWNLOAD/);
});
