import { app, BrowserWindow, Menu, Tray, nativeImage, shell, nativeTheme } from 'electron';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { startServer } from '../src/server.js';
import { magnetFromArgv } from '../src/magnet.js';

const here = path.dirname(fileURLToPath(import.meta.url));
// Linux dock simgesi .desktop dosyasındaki StartupWMClass ile eşleşir.
// Sınıf "electron" kalırsa görev çubuğu boş ya da çark simgesi gösterir.
if (process.platform === 'linux' && !app.commandLine.hasSwitch('class')) {
  app.commandLine.appendSwitch('class', 'torrent-bul');
}
let server;
let win = null;
let tray = null;
let quitting = false;
const pendingMagnets = []; // sunucu hazır olmadan gelen bağlantılar

// Tek örnek: ikinci açılış (ör. tarayıcıda magnet'e tıklamak) mevcut pencereye gelir.
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on('second-instance', (_e, argv) => {
    const magnet = magnetFromArgv(argv);
    if (magnet) addMagnet(magnet);
    showWindow();
  });
  // macOS bağlantıyı argv yerine open-url ile verir (uygulama açılmadan da gelebilir).
  app.on('open-url', (e, url) => {
    e.preventDefault();
    const magnet = magnetFromArgv([url]);
    if (magnet) addMagnet(magnet);
  });
  // Geliştirmede (npm start) protokol kaydı yapılmaz; yoksa her çalıştırma işletim sistemini kirletir.
  if (app.isPackaged) app.setAsDefaultProtocolClient('magnet');

  app.whenReady().then(async () => {
    await createWindow();
    const first = magnetFromArgv(process.argv);
    if (first) addMagnet(first);
  });
}

function addMagnet(magnet) {
  if (!server) return void pendingMagnets.push(magnet);
  server.addDownload({ magnet, source: 'Magnet bağlantısı' }).catch((err) => console.error('[magnet]', err.message));
  showWindow();
}

function showWindow() {
  if (!win) return void (app.isReady() && createWindow());
  if (win.isMinimized()) win.restore();
  win.show();
  win.focus();
}

function ensureTray() {
  if (tray) return;
  tray = new Tray(nativeImage.createFromPath(path.join(here, 'tray.png')));
  tray.setToolTip('Torrent Bul');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Aç', click: showWindow },
    { label: 'Çıkış', click: () => { quitting = true; app.quit(); } },
  ]));
  tray.on('click', showWindow);
}

async function createWindow() {
  server ??= await startServer({
    dataDir: app.getPath('userData'),
    openPath: (p) => shell.openPath(p),
  });
  win = new BrowserWindow({
    width: 1180,
    height: 800,
    minWidth: 760,
    minHeight: 560,
    title: 'Torrent Bul',
    // Simge verilmezse Linux'ta pencere ve görev çubuğunda genel çark simgesi çıkar.
    icon: path.join(here, 'icon.png'),
    backgroundColor: nativeTheme.shouldUseDarkColors ? '#141414' : '#f3f3f1',
    titleBarStyle: process.platform === 'darwin' ? 'hiddenInset' : 'default',
    autoHideMenuBar: true,
    webPreferences: { contextIsolation: true, sandbox: true },
  });
  // Dış bağlantılar (ör. archive.org sayfası) varsayılan tarayıcıda açılsın.
  win.webContents.setWindowOpenHandler(({ url }) => {
    if (/^https?:\/\//.test(url)) shell.openExternal(url);
    return { action: 'deny' };
  });
  win.webContents.on('will-navigate', (e, url) => {
    if (!url.startsWith(server.url)) { e.preventDefault(); shell.openExternal(url); }
  });
  // İsteğe bağlı: kapatınca tepsiye gizlen. Varsayılan davranış (gerçekten kapat) değişmez.
  win.on('close', (e) => {
    if (quitting || !server?.getSettings().closeToTray) return;
    e.preventDefault();
    ensureTray();
    win.hide();
  });
  win.on('closed', () => { win = null; });
  await win.loadURL(server.url);
  for (const magnet of pendingMagnets.splice(0)) addMagnet(magnet);
}

app.on('activate', () => { if (BrowserWindow.getAllWindows().length === 0) createWindow(); else showWindow(); });
app.on('window-all-closed', () => { if (process.platform !== 'darwin') app.quit(); });
app.on('before-quit', async (e) => {
  quitting = true;
  if (!server) return;
  e.preventDefault();
  const s = server;
  server = null;
  await s.close().catch(() => {});
  app.quit();
});
