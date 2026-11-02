# Torrent Bul

Film, dizi, müzik veya kitap ara, indir, isterken izle. İndirmeyi [qBittorrent](https://www.qbittorrent.org) yapar. Uygulama onu kendi profiliyle açar; senin qBittorrent ayarına ve listene dokunmaz. Arayüz Türkçe.

Bu sürüm geliştirici kurulumu. Hazır kurulum dosyası yok: Node.js 20, qBittorrent ve `npm start` gerekir.

## Ne yapar

- Arama kutusuna başlık yaz, kategoriyi seç, indir.
- Film ve dizide puan ve afiş gelir (Cinemeta, anahtarsız). İstersen Ayarlar’a ücretsiz OMDb anahtarı yazarsın; oy sayısı da görünür.
- Çok dosyalı torrentte inecek dosyaları seçersin. Film diye gelen program dosyası (.exe ve benzeri) inmez; program yalnız Yazılım kategorisinden iner.
- Aynı anda sınırlı sayıda indirme çalışır, kalanı sırada bekler. Yarım kalan iş uygulamayı kapatıp açınca sürer.
- Bitmiş videolar ana sayfada “Son izlenen” listesinde durur. Kaldığın yerden açılır, izlendi diye işaretlenebilir.
- İstersen Jackett veya Prowlarr bağlarsın. Uygulama kendi başına bir torrent sitesine girmez.

Yalnızca indirme hakkın olanı indir.

## Kaynaklar

| Kaynak | Varsayılan | Not |
| --- | --- | --- |
| Internet Archive | Açık | Kamu malı ve özgür lisans. Güncel film burada yoktur. |
| Torznab (Jackett / Prowlarr) | Kapalı | Ayarlardan adres ve API anahtarı. Sonuçta paylaşan sayısı da görünür. |

Jackett, Linux:

```bash
cd ~ && wget https://github.com/Jackett/Jackett/releases/latest/download/Jackett.Binaries.LinuxAMDx64.tar.gz
tar -xzf Jackett.Binaries.LinuxAMDx64.tar.gz && cd Jackett && sudo ./install_service_systemd.sh
```

1. `http://127.0.0.1:9117` adresini aç, **+ Add indexer** ile site ekle.
2. Sağ üstteki API anahtarını kopyala.
3. Torrent Bul’da **Ayarlar → + Kendi indeksleyicini ekle**, anahtarı yapıştır, kaydet.

Üyesi olduğun özel site de Jackett listesindedir. Bazı Cloudflare’lı siteler Jackett’ta [FlareSolverr](https://github.com/FlareSolverr/FlareSolverr) ister.

Yeni bir kaynak için `src/providers/` altına `search(params)` fonksiyonu olan bir modül yazıp `src/search.js` listesine ekle.

## Kurulum

```bash
sudo apt install qbittorrent-nox   # Linux, penceresiz sürüm
npm install
npm start
```

Linux’ta menüye almak için bir kez: `npm run kisayol`

Windows ve macOS’ta [qbittorrent.org](https://www.qbittorrent.org/download) kurulumu yeter. Başka bir dizindeyse yolu `QBITTORRENT_BIN` ile ver.

Tarayıcıda: `npm run web` → `http://127.0.0.1:4517`

Kurulum dosyası üretmek (henüz yayınlanmıyor, çıktı `dist/`):

```bash
npm run dist:win
npm run dist:mac
npm run dist:linux
```

İndirmeler varsayılan olarak `İndirilenler/Torrent` klasörüne gider. Ayarlardan değişir.

## Geliştirme

```bash
npm test
```

Testler Türkçe tespitini, Archive ve Torznab sorgularını, durum dosyalarını, dosya seçimini, kütüphaneyi, sırayı ve gerçek qBittorrent ile bir indirmeyi çalıştırır. `qbittorrent-nox` gerekir. Her push ve pull request’te GitHub Actions koşar.

```
electron/main.js        Masaüstü penceresi
bin/web.js              Tarayıcı modu
src/server.js           Yerel API (yalnız 127.0.0.1, oturum anahtarı)
src/downloads.js        İndirme yöneticisi
src/qbittorrent.js      qBittorrent’i başlatır, Web API ile sürer
src/library.js          İzleme geçmişi
src/search.js           Kaynakları birleştirir
src/ranking.js          Türkçe tespiti ve sıralama
src/providers/          Internet Archive ve Torznab
public/                 Arayüz
```

## Lisans

[MIT](LICENSE)
