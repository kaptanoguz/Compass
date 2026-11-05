import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { installDesktopEntry } from '../src/desktop-entry.js';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

test('menü kısayolu Compass adıyla kurulur, eski Torrent Bul kaydı silinir, güncelse yeniden yazılmaz', () => {
  const data = fs.mkdtempSync(path.join(os.tmpdir(), 'compass-desktop-'));
  fs.mkdirSync(path.join(data, 'applications'), { recursive: true });
  fs.mkdirSync(path.join(data, 'icons', 'hicolor', '512x512', 'apps'), { recursive: true });
  fs.writeFileSync(path.join(data, 'applications', 'torrent-bul.desktop'), '[Desktop Entry]\nName=Torrent Bul\n');
  fs.writeFileSync(path.join(data, 'icons', 'hicolor', '512x512', 'apps', 'torrent-bul.png'), 'eski');

  const first = installDesktopEntry({ root, electron: '/opt/electron', data });
  assert.equal(first.changed, true);
  const text = fs.readFileSync(path.join(data, 'applications', 'compass.desktop'), 'utf8');
  assert.match(text, /^Name=Compass$/m);
  assert.match(text, /^StartupWMClass=compass$/m);
  assert.match(text, /--class=compass/);
  assert.ok(fs.existsSync(path.join(data, 'icons', 'hicolor', 'scalable', 'apps', 'compass.svg')));
  assert.ok(fs.existsSync(path.join(data, 'icons', 'hicolor', '48x48', 'apps', 'compass.png')));
  assert.ok(!fs.existsSync(path.join(data, 'applications', 'torrent-bul.desktop')));
  assert.ok(!fs.existsSync(path.join(data, 'icons', 'hicolor', '512x512', 'apps', 'torrent-bul.png')));

  assert.equal(installDesktopEntry({ root, electron: '/opt/electron', data }).changed, false);
});
