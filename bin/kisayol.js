#!/usr/bin/env node
// Linux'ta uygulama menüsüne "Compass" kısayolu ve simgesi ekler.
// GNOME/KDE pencereyi bu .desktop dosyasıyla eşleştirir; dosya yoksa görev
// çubuğunda genel çark simgesi görünür. Yalnızca kullanıcının kendi klasörüne yazar.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

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

const data = process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share');
const appDir = path.join(data, 'applications');
fs.mkdirSync(appDir, { recursive: true });
// Dock her boyutta net görünsün: vektör simge ve hazır boyutlar birlikte kurulur;
// yalnızca 512'lik simge olunca bazı masaüstleri küçük bir kopyayı büyütüp bulanıklaştırıyor.
const install = (from, size, ext) => {
  const dir = path.join(data, 'icons', 'hicolor', size, 'apps');
  fs.mkdirSync(dir, { recursive: true });
  fs.copyFileSync(from, path.join(dir, `compass.${ext}`));
};
install(path.join(root, 'public', 'icon.svg'), 'scalable', 'svg');
for (const f of fs.readdirSync(path.join(root, 'build', 'icons'))) {
  if (/^\d+x\d+\.png$/.test(f)) install(path.join(root, 'build', 'icons', f), f.slice(0, -4), 'png');
}

const q = (s) => `"${s.replace(/(["`$\\])/g, '\\$1')}"`;
const iconPng = path.join(root, 'build', 'icon.png');
// Icon=tema adı bazı masaüstlerinde önbelleğe takılıp boş görünür.
// Mutlak png yolu dock'ta hemen çizilir.
const desktop = `[Desktop Entry]
Type=Application
Version=1.0
Name=Compass
GenericName=Torrent arama
Comment=Film, dizi ve kitap ara, indir, izle
Exec=${q(electron)} ${q(root)} --class=compass %U
Icon=${iconPng}
Terminal=false
Categories=AudioVideo;Network;FileTransfer;
Keywords=torrent;film;dizi;video;
StartupNotify=true
StartupWMClass=compass
`;
const file = path.join(appDir, 'compass.desktop');
// Uygulamanın eski adıyla ("Torrent Bul") kurulmuş kısayol ve simgeler kaldırılır; menüde iki kayıt kalmasın.
fs.rmSync(path.join(appDir, 'torrent-bul.desktop'), { force: true });
for (const size of fs.readdirSync(path.join(data, 'icons', 'hicolor'))) {
  for (const ext of ['png', 'svg']) fs.rmSync(path.join(data, 'icons', 'hicolor', size, 'apps', `torrent-bul.${ext}`), { force: true });
}
fs.writeFileSync(file, desktop);
fs.chmodSync(file, 0o755);
// Menünün hemen yenilenmesi için; araç yoksa sorun değil.
spawnSync('update-desktop-database', [appDir], { stdio: 'ignore' });
spawnSync('gtk-update-icon-cache', ['-f', '-t', path.join(data, 'icons', 'hicolor')], { stdio: 'ignore' });

console.log(`Kısayol eklendi: ${file}`);
console.log('Uygulama menüsünde "Compass" diye aratabilirsin. Klasörü taşırsan bu komutu yeniden çalıştır.');
