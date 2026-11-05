# Compass

Find a movie, series, album, or book, download it with one click, and start watching while it finishes.

![Compass home screen](docs/home.png)

Compass is a simple desktop app. You type what you're looking for, press **Ara** (Search), and press **İndir** (Download). That's it. The app's menus are in Turkish.

> Only download what you have the right to download. Out of the box, Compass searches the Internet Archive, which has public domain and freely licensed works.

## What you can do

**Search by name, topic, or genre.** Results show the year, genre, size, and a rating. Turkish results come first, and you can turn that off.

![Search results](docs/results.png)

**Download with one click.** Downloads show progress, speed, and time left. If you close the app, unfinished downloads continue next time you open it.

![Downloads panel](docs/downloads.png)

**Watch right away.** The built-in player can start before the download is done. It remembers where you stopped, and you can load your own subtitles.

![Built-in player](docs/player.png)

**Stay safe.** If a "movie" turns out to be a program (like an `.exe` file), Compass won't download it.

## Install

1. Install **qBittorrent**, the free program that does the actual downloading. Compass runs it in the background for you, so you never need to open it.
   - Windows: download it from [qbittorrent.org](https://www.qbittorrent.org/download).
   - Linux: open a terminal and run `sudo apt install qbittorrent-nox`.
2. Download Compass from the [releases page](https://github.com/kaptanoguz/torrent/releases) and install it.
3. Open **Compass** from your application menu.

Your downloads go to the `Downloads/Torrent` folder. You can change this in Settings (the gear icon).

## Want more search results?

By default Compass only searches the Internet Archive, so you won't find new movies there. If you'd like to search other sites, you can connect **Jackett**, a free tool that lets Compass search them for you:

1. Install [Jackett](https://github.com/Jackett/Jackett) and open `http://127.0.0.1:9117`.
2. Click **+ Add indexer** and pick the sites you want.
3. Copy the **API Key** shown at the top right.
4. In Compass, open Settings (gear icon), click **+ Kendi indeksleyicini ekle** (Add your own indexer), paste the key, and save.

Once Jackett is connected, Compass also shows genre shortcuts on the home screen.

## For developers

Compass is an Electron app with a small local server. It needs Node.js 20 or newer and qBittorrent.

```bash
npm install
npm start        # desktop app
npm run web      # same app in your browser at http://127.0.0.1:4517
npm test         # needs qbittorrent-nox
npm run dist:linux   # or dist:win, builds an installer into dist/
```

New search sources go in `src/providers/` and are registered in `src/search.js`.

## License

[MIT](LICENSE)
