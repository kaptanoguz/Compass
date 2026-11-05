// Linux'ta uygulama menüsüne "Compass" kısayolu ve simgesi ekler.
// GNOME/KDE pencereyi bu .desktop dosyasıyla eşleştirir; dosya yoksa dock'ta
// genel çark simgesi görünür. Yalnızca kullanıcının kendi klasörüne yazar.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const q = (s) => `"${s.replace(/(["`$\\])/g, '\\$1')}"`;

// Icon= tema adı bazı masaüstlerinde önbelleğe takılıp boş görünür; mutlak png yolu dock'ta hemen çizilir.
export function desktopEntry({ root, electron }) {
  return `[Desktop Entry]
Type=Application
Version=1.0
Name=Compass
GenericName=Torrent arama
Comment=Film, dizi ve kitap ara, indir, izle
Exec=${q(electron)} ${q(root)} --class=compass %U
Icon=${path.join(root, 'build', 'icon.png')}
Terminal=false
Categories=AudioVideo;Network;FileTransfer;
Keywords=torrent;film;dizi;video;pusula;
StartupNotify=true
StartupWMClass=compass
`;
}

// Kısayol ve simgeler güncelse hiçbir şey yazılmaz; uygulama her açılışta çağırabilir.
export function installDesktopEntry({ root, electron, data = process.env.XDG_DATA_HOME || path.join(os.homedir(), '.local', 'share') }) {
  const appDir = path.join(data, 'applications');
  const icons = path.join(data, 'icons', 'hicolor');
  const file = path.join(appDir, 'compass.desktop');
  const text = desktopEntry({ root, electron });
  const svg = fs.readFileSync(path.join(root, 'public', 'icon.svg'));
  const svgTarget = path.join(icons, 'scalable', 'apps', 'compass.svg');
  const same = (p, content) => { try { return fs.readFileSync(p).equals(Buffer.from(content)); } catch { return false; } };
  if (same(file, text) && same(svgTarget, svg)) return { file, changed: false };

  // Dock her boyutta net görünsün: vektör simge ve hazır boyutlar birlikte kurulur.
  const install = (from, size, ext) => {
    const dir = path.join(icons, size, 'apps');
    fs.mkdirSync(dir, { recursive: true });
    fs.copyFileSync(from, path.join(dir, `compass.${ext}`));
  };
  install(path.join(root, 'public', 'icon.svg'), 'scalable', 'svg');
  for (const f of fs.readdirSync(path.join(root, 'build', 'icons'))) {
    if (/^\d+x\d+\.png$/.test(f)) install(path.join(root, 'build', 'icons', f), f.slice(0, -4), 'png');
  }
  fs.mkdirSync(appDir, { recursive: true });
  fs.writeFileSync(file, text);
  fs.chmodSync(file, 0o755);
  // Uygulamanın eski adıyla ("Torrent Bul") kurulmuş kısayol ve simgeler kaldırılır; menüde iki kayıt kalmasın.
  fs.rmSync(path.join(appDir, 'torrent-bul.desktop'), { force: true });
  for (const size of fs.readdirSync(icons)) {
    for (const ext of ['png', 'svg']) fs.rmSync(path.join(icons, size, 'apps', `torrent-bul.${ext}`), { force: true });
  }
  // Menünün ve dock'un hemen yenilenmesi için; araç yoksa sorun değil.
  spawnSync('update-desktop-database', [appDir], { stdio: 'ignore' });
  spawnSync('gtk-update-icon-cache', ['-f', '-t', icons], { stdio: 'ignore' });
  return { file, changed: true };
}
