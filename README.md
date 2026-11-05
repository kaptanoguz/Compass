# Compass

Search for a movie, series, album, or book, download it, and play it while it finishes. Downloads run in [qBittorrent](https://www.qbittorrent.org). The app starts qBittorrent with its own profile and does not touch your settings or torrent list. The interface is Turkish.

This is a developer build. There is no installer yet. You need Node.js 20, qBittorrent, and `npm start`.

## What it does

- Type a title, pick a category, download.
- Movies and series get a rating and poster from Cinemeta, with no API key. An optional free OMDb key in Settings also shows the vote count.
- In a multi-file torrent you choose which files to download. An executable shipped as a movie (`.exe` and similar) is not downloaded. Programs only download from the Software category.
- A few downloads run at once; the rest wait. Unfinished downloads resume after you close and reopen the app.
- Finished videos stay on the home page under Son izlenen. You can resume and mark one as watched.
- Add Jackett or Prowlarr if you want other sites. The app does not ship with any torrent site of its own.

Only download what you have the right to download.

## Sources

| Source | Default | Note |
| --- | --- | --- |
| Internet Archive | On | Public domain and freely licensed works. Current commercial films are not here. |
| Torznab (Jackett / Prowlarr) | Off | Address and API key in Settings. Seeder counts are shown. |

Jackett on Linux:

```bash
cd ~ && wget https://github.com/Jackett/Jackett/releases/latest/download/Jackett.Binaries.LinuxAMDx64.tar.gz
tar -xzf Jackett.Binaries.LinuxAMDx64.tar.gz && cd Jackett && sudo ./install_service_systemd.sh
```

1. Open `http://127.0.0.1:9117` and add sites with **+ Add indexer**.
2. Copy the API key at the top right.
3. In Compass open **Ayarlar**, choose **+ Kendi indeksleyicini ekle**, paste the key, and save.

Private trackers you belong to are in Jackett's list too. Some Cloudflare-protected sites also need [FlareSolverr](https://github.com/FlareSolverr/FlareSolverr).

To add a source in code, put a module with a `search(params)` function under `src/providers/` and register it in `src/search.js`.

The genre list and the +18 category stay hidden until a Torznab source is enabled. Internet Archive alone does not make those useful.

## Install

Installers for Linux and Windows are on the [releases page](https://github.com/kaptanoguz/torrent/releases). qBittorrent is not inside the installer. Linux: `sudo apt install qbittorrent-nox`. Windows: install qBittorrent from [qbittorrent.org](https://www.qbittorrent.org/download), then run the setup exe. The app looks for `qbittorrent.exe` in `Program Files`.

From source:

```bash
sudo apt install qbittorrent-nox   # Linux, no window
npm install
npm run kisayol                    # Linux only, if you did not use the installer
```

Open **Compass** from the application menu. The packaged build already has a menu entry and a dock icon, so `npm run kisayol` is only for a source checkout. `npm start` still works.

In a browser: `npm run web` opens `http://127.0.0.1:4517`.

Build installers locally (output in `dist/`, not signed):

```bash
npm run dist:win
npm run dist:linux
```

Downloads go to `Downloads/Torrent` by default. Change the folder in Settings.

## Development

```bash
npm test
```

Tests cover Turkish detection, Archive and Torznab queries, state files, file selection, the library, the queue, and one real download through qBittorrent. `qbittorrent-nox` is required. GitHub Actions runs them on every push and pull request.

```
electron/main.js        Desktop window
bin/web.js              Browser mode
src/server.js           Local API (127.0.0.1 only, session key)
src/downloads.js        Download manager
src/qbittorrent.js      Starts qBittorrent and drives the Web API
src/library.js          Watch history
src/search.js           Combines sources
src/ranking.js          Turkish detection and ranking
src/providers/          Internet Archive and Torznab
public/                 Interface
```

## License

[MIT](LICENSE)
