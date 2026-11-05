#!/usr/bin/env node
// Linux'ta uygulama menüsüne "Compass" kısayolu ve simgesi ekler.
// Uygulama (npm start) bunu açılışta kendisi de yapar; bu komut elle yenilemek içindir.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { installDesktopEntry } from '../src/desktop-entry.js';

if (process.platform !== 'linux') {
  console.log('Bu komut yalnızca Linux içindir. Windows ve macOS kurulum dosyası simgeyi kendisi ekler.');
  process.exit(0);
}

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const electron = path.join(root, 'node_modules', '.bin', 'electron');
if (!fs.existsSync(electron)) {
  console.error('Önce "npm install" çalıştır.');
  process.exit(1);
}

const { file } = installDesktopEntry({ root, electron });
console.log(`Kısayol eklendi: ${file}`);
console.log('Uygulama menüsünde "Compass" diye aratabilirsin. Klasörü taşırsan bu komutu yeniden çalıştır.');
