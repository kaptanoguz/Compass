// Testler için gerçek bir BitTorrent eşi: ayrı bir qBittorrent örneği verilen
// dosyaları paylaşır. Web kaynağından farklı olarak parçaları istenen sırayla verir.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import createTorrent from 'create-torrent';
import { launchQbittorrent } from '../../src/qbittorrent.js';

export async function startSeeder(files, { name, uploadLimit = 0 } = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'compass-seeder-'));
  const data = path.join(dir, 'data');
  fs.mkdirSync(path.join(data, name), { recursive: true });
  for (const [file, buf] of Object.entries(files)) fs.writeFileSync(path.join(data, name, file), buf);
  const input = Object.entries(files).map(([file, buf]) => Object.assign(Buffer.from(buf), { name: file }));
  const torrent = Buffer.from(await new Promise((resolve, reject) =>
    createTorrent(input, { name, pieceLength: 16384, announceList: [], private: false }, (e, t) => (e ? reject(e) : resolve(t)))));
  const qb = await launchQbittorrent({ profileDir: path.join(dir, 'profile') });
  const form = new FormData();
  form.append('torrents', new Blob([torrent]), 'seed.torrent');
  form.append('savepath', data);
  form.append('skip_checking', 'true');
  await qb.api.post('torrents/add', form);
  if (uploadLimit) await qb.api.post('transfer/setUploadLimit', { limit: String(uploadLimit) });
  const { listen_port: port } = await qb.api.get('app/preferences');
  const parseTorrent = (await import('parse-torrent')).default;
  const { infoHash } = await parseTorrent(torrent);
  return {
    torrent,
    infoHash,
    magnet: `magnet:?xt=urn:btih:${infoHash}&dn=${encodeURIComponent(name)}&x.pe=127.0.0.1:${port}`,
    stop: () => qb.stop(),
  };
}
