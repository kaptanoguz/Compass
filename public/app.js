const TOKEN = document.querySelector('meta[name="app-token"]').content;
const $ = (id) => document.getElementById(id);

const ICONS = {
  movies: '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2.5"/><path d="m10 9.5 4.5 2.5L10 14.5z"/></svg>',
  audio: '<svg viewBox="0 0 24 24"><path d="M9 18V6l11-2v12"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/></svg>',
  texts: '<svg viewBox="0 0 24 24"><path d="M4 5.5A2.5 2.5 0 0 1 6.5 3H20v16H6.5A2.5 2.5 0 0 0 4 21.5z"/><path d="M4 21.5V5.5M8 7h8"/></svg>',
  software: '<svg viewBox="0 0 24 24"><rect x="3" y="4" width="18" height="14" rx="2"/><path d="M8 21h8M12 18v3M8 9l-2 2 2 2M16 9l2 2-2 2"/></svg>',
  pause: '<svg viewBox="0 0 24 24"><path d="M9 5v14M15 5v14"/></svg>',
  play: '<svg viewBox="0 0 24 24"><path d="m7 4 13 8-13 8z"/></svg>',
  folder: '<svg viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>',
  trash: '<svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/></svg>',
  star: '<svg viewBox="0 0 24 24" class="star"><path d="m12 3 2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9z"/></svg>',
  x: '<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6 6 18"/></svg>',
  tv: '<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="13" rx="2"/><path d="m10.5 9.5 4 2-4 2z"/><path d="M8 21h8"/></svg>',
  file: '<svg viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>',
};

const store = {
  get(key, fallback) {
    try { return localStorage.getItem(key) ?? fallback; } catch { return fallback; }
  },
  set(key, value) {
    try { localStorage.setItem(key, value); } catch { /* gizli pencere vb. */ }
  },
};

const state = {
  category: 'all',
  genre: '', // ana sayfadaki tür kartından gelen sunucu araması
  genreFilter: '', // sonuçlar içinde istemci tarafı süzme
  sort: store.get('sort', 'year-desc'),
  trFirst: store.get('trFirst', '1') === '1',
  genres: {},
  results: [],
  downloads: [],
  searchSeq: 0,
};

async function api(path, options = {}) {
  const res = await fetch(`/api/${path}`, {
    ...options,
    headers: { 'X-App-Token': TOKEN, ...(options.body ? { 'Content-Type': 'application/json' } : {}) },
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Bir şeyler ters gitti.');
  return data;
}

function esc(s) {
  return String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
}

function formatBytes(n) {
  if (!n) return '';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.min(Math.floor(Math.log(n) / Math.log(1024)), u.length - 1);
  return `${(n / 1024 ** i).toLocaleString('tr-TR', { maximumFractionDigits: i >= 3 ? 2 : 1 })} ${u[i]}`;
}

function formatTime(ms) {
  if (ms == null) return '';
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} sn`;
  if (s < 3600) return `${Math.round(s / 60)} dk`;
  const h = Math.floor(s / 3600);
  return `${h} sa ${Math.round((s % 3600) / 60)} dk`;
}

function toast(message, kind = '') {
  const el = document.createElement('div');
  el.className = `toast ${kind}`;
  el.textContent = message;
  $('toasts').append(el);
  setTimeout(() => el.remove(), 3200);
}

/* ---------- Kategori ve türler ---------- */

async function loadMeta() {
  const meta = await api('meta');
  state.genres = meta.genres;
  const ids = (meta.providers || []).map((p) => p.id).filter(Boolean);
  state.hasIndexer = ids.some((id) => id !== 'archive');
  const categories = Object.entries(meta.categories).filter(([k]) => state.hasIndexer || k !== 'adult');
  if (!categories.some(([k]) => k === state.category)) state.category = 'all';
  $('category').innerHTML = categories
    .map(([k, label]) => `<option value="${k}">${esc(label)}</option>`)
    .join('');
  $('category').value = state.category;
  $('browse').hidden = !state.hasIndexer;
  if ($('source-note')) {
    $('source-note').hidden = state.hasIndexer;
    if (!ids.length) $('source-note').textContent = 'Etkin arama kaynağı yok. Ayarlar’dan bir kaynak aç.';
  }
  $('genres').innerHTML = Object.entries(meta.genres)
    .map(([g, label]) => `<button type="button" class="genre-card" data-genre="${g}"><span>${esc(label)}</span></button>`)
    .join('');
}

$('category').addEventListener('change', async () => {
  const next = $('category').value;
  if (next === 'adult' && store.get('ageOk') !== '1') {
    $('age-gate').returnValue = '';
    $('age-gate').showModal();
    const ok = await new Promise((resolve) => $('age-gate').addEventListener('close', () => resolve($('age-gate').returnValue === 'ok'), { once: true }));
    if (!ok) { $('category').value = state.category; return; }
    store.set('ageOk', '1');
  }
  state.category = next;
  document.body.classList.toggle('adult-mode', next === 'adult');
  if (hasQuery() || next === 'adult') search();
});

$('genres').addEventListener('click', (e) => {
  const card = e.target.closest('[data-genre]');
  if (!card) return;
  state.genre = card.dataset.genre;
  search();
});

$('genre-pill').addEventListener('click', () => {
  state.genre = '';
  if ($('q').value.trim()) search();
  else goHome();
});

function hasQuery() {
  return $('q').value.trim() || state.genre;
}

/* ---------- Arama ---------- */

$('search-form').addEventListener('submit', (e) => {
  e.preventDefault();
  search();
});

$('home-link').addEventListener('click', (e) => {
  e.preventDefault();
  $('q').value = '';
  state.genre = '';
  goHome();
  $('q').focus();
});

function goHome() {
  state.searchSeq++;
  loadLibrary();
  $('main').classList.add('is-home');
  $('results').innerHTML = '';
  $('toolbar').hidden = true;
  $('notice').hidden = true;
  $('search-btn').disabled = false;
}

// Aramada süreye sığmayan afiş ve IMDb puanları sonradan gelir; sonuçlar beklemeden görünür.
async function fillRatings(seq) {
  const items = state.results.filter((r) => !r.imdbUrl && (r.category === 'movies' || r.category === 'tv'))
    .slice(0, 40).map(({ id, title, year, category, imdbId }) => ({ id, title, year, category, imdbId }));
  if (!items.length) return;
  const { ratings } = await api('ratings', { method: 'POST', body: JSON.stringify({ items }) }).catch(() => ({}));
  if (seq !== state.searchSeq || !ratings || !Object.keys(ratings).length) return;
  for (const r of state.results) {
    const info = ratings[r.id];
    if (!info) continue;
    if (info.rating) r.rating = info.rating;
    if (info.poster) r.poster = info.poster;
    if (!r.year && info.year) r.year = info.year;
    r.imdbUrl = info.imdbUrl;
    r.genres = [...new Set([...(r.genres || []), ...info.genres])];
  }
  renderResults();
}

async function search() {
  const q = $('q').value.trim();
  if (/^magnet:\?/i.test(q)) return showMagnet(q);
  if (!q && !state.genre && state.category !== 'adult') return goHome();

  const seq = ++state.searchSeq;
  $('main').classList.remove('is-home');
  $('notice').hidden = true;
  $('toolbar').hidden = true;
  $('results').innerHTML = Array.from({ length: 5 }, () => '<li class="skeleton"></li>').join('');
  $('search-btn').disabled = true;

  try {
    const params = new URLSearchParams({ q, category: state.category, genre: state.genre });
    const data = await api(`search?${params}`);
    if (seq !== state.searchSeq) return;
    state.results = data.results;
    state.genreFilter = '';
    state.errors = data.errors;
    renderResults();
    fillRatings(seq);
  } catch (err) {
    if (seq !== state.searchSeq) return;
    $('results').innerHTML = '';
    showNotice(`<strong>Arama yapılamadı.</strong> ${esc(err.message)}`);
  } finally {
    if (seq === state.searchSeq) $('search-btn').disabled = false;
  }
}

function showNotice(html, cls = '') {
  $('notice').className = `notice ${cls}`;
  $('notice').innerHTML = html;
  $('notice').hidden = false;
}

function showMagnet(magnet) {
  $('main').classList.remove('is-home');
  $('results').innerHTML = '';
  $('toolbar').hidden = true;
  showNotice(
    `<span><strong>Magnet bağlantısı algılandı.</strong> Doğrudan indirmeye başlayabilirsin.</span>
     <button class="primary-btn" type="button" id="magnet-dl">İndir</button>`,
    'magnet',
  );
  $('magnet-dl').addEventListener('click', () => startDownload({ magnet }));
}

/* ---------- Sıralama ve süzme ---------- */

const popularity = (r) => (r.seeders ?? 0) * 50 + (r.downloads ?? 0);
const genreLabel = (key) => state.genres[key] || key;

const SORTERS = {
  'year-desc': (a, b) => (b.year ?? -Infinity) - (a.year ?? -Infinity),
  'year-asc': (a, b) => (a.year ?? Infinity) - (b.year ?? Infinity),
  rating: (a, b) => (b.rating?.value ?? -1) - (a.rating?.value ?? -1),
  popular: (a, b) => popularity(b) - popularity(a),
  size: (a, b) => (b.size ?? 0) - (a.size ?? 0),
  genre: (a, b) => {
    const ga = a.genres?.[0] ? genreLabel(a.genres[0]) : '￿';
    const gb = b.genres?.[0] ? genreLabel(b.genres[0]) : '￿';
    return ga.localeCompare(gb, 'tr');
  },
  relevance: () => 0, // sunucunun sırası korunur
};

function visibleResults() {
  const list = state.results
    .map((r, i) => ({ r, i }))
    .filter(({ r }) => !state.genreFilter || r.genres?.includes(state.genreFilter));
  const cmp = SORTERS[state.sort] || SORTERS['year-desc'];
  list.sort((a, b) => {
    if (state.trFirst && a.r.turkish !== b.r.turkish) return a.r.turkish ? -1 : 1;
    return cmp(a.r, b.r) || a.i - b.i;
  });
  return list.map(({ r }) => r);
}

function renderGenreFilter() {
  const counts = new Map();
  for (const r of state.results) for (const g of r.genres || []) counts.set(g, (counts.get(g) || 0) + 1);
  const options = [...counts.entries()].sort((a, b) => b[1] - a[1]);
  $('genre-filter').innerHTML = `<option value="">Tümü (${state.results.length})</option>`
    + options.map(([g, n]) => `<option value="${g}">${esc(genreLabel(g))} (${n})</option>`).join('');
  $('genre-filter').value = state.genreFilter;
  $('genre-filter').disabled = options.length === 0;
}

$('sort').value = state.sort;
$('tr-first').checked = state.trFirst;
$('sort').addEventListener('change', () => {
  state.sort = $('sort').value;
  store.set('sort', state.sort);
  renderList();
});
$('tr-first').addEventListener('change', () => {
  state.trFirst = $('tr-first').checked;
  store.set('trFirst', state.trFirst ? '1' : '0');
  renderList();
});
$('genre-filter').addEventListener('change', () => {
  state.genreFilter = $('genre-filter').value;
  renderList();
});

/* ---------- Sonuç listesi ---------- */

function healthPill(r) {
  if (r.webSeed && r.seeders == null) return '<span class="pill health" title="Sunucudan doğrudan iner">Hızlı kaynak</span>';
  if (r.seeders == null) return '';
  const cls = r.seeders >= 10 ? '' : r.seeders > 0 ? 'low' : 'dead';
  return `<span class="pill health ${cls}" title="Paylaşan kişi sayısı">${r.seeders.toLocaleString('tr-TR')} paylaşan</span>`;
}

function ratingBadge(r) {
  if (!r.rating) return '';
  const value = r.rating.value.toLocaleString('tr-TR', { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  const votes = r.rating.votes ? ` · ${r.rating.votes.toLocaleString('tr-TR')} oy` : '';
  const body = `${ICONS.star}<strong>${value}</strong><small>${esc(r.rating.source)}</small>`;
  return r.imdbUrl
    ? `<a class="rating" href="${esc(r.imdbUrl)}" target="_blank" rel="noopener" title="${esc(r.rating.source)} puanı${votes}">${body}</a>`
    : `<span class="rating" title="${esc(r.rating.source)} puanı (10 üzerinden)${votes}">${body}</span>`;
}

function resultButton(r) {
  const d = findDownload(r);
  if (!d) return `<button class="primary-btn" type="button" data-dl="${esc(r.id)}">İndir</button>`;
  if (d.status === 'done' && d.missing) return `<button class="primary-btn" type="button" data-dl="${esc(r.id)}" title="Dosyalar bulunamadı">Tekrar indir</button>`;
  if (d.status === 'done' && d.videos?.some((v) => v.playableInBrowser)) return `<button class="primary-btn done" type="button" data-watch="${d.id}">İzle</button>`;
  if (d.status === 'done' && d.videos?.length) return `<button class="primary-btn done" type="button" data-external="${d.id}" title="Bu biçim yerleşik oynatıcıda açılmıyor">Oynat</button>`;
  if (d.status === 'done') return `<button class="primary-btn done" type="button" data-open="${d.id}">Aç</button>`;
  if (d.status === 'error') return `<button class="primary-btn" type="button" data-resume="${d.id}">Tekrar dene</button>`;
  if (d.status === 'picking') return `<button class="primary-btn" type="button" data-pick="${d.id}">Dosya seç</button>`;
  if (d.status === 'paused') return `<button class="primary-btn" type="button" data-resume="${d.id}">Devam et</button>`;
  return `<button class="primary-btn going" type="button" data-show-downloads>%${Math.floor(d.progress * 100)}</button>`;
}

// Aynı torrent farklı magnet yazımıyla da gelebilir; infohash ile de eşleştir.
function findDownload(r) {
  const hash = (r.infoHash || magnetHash(r.magnet) || '').toLowerCase();
  return state.downloads.find((d) => d.key === (r.magnet || r.torrentUrl) || (hash && d.infoHash === hash));
}

function magnetHash(magnet) {
  const m = /[?&]xt=urn:btih:([0-9a-f]{40}|[a-z2-7]{32})(?=&|$)/i.exec(magnet || '');
  if (!m) return null;
  if (m[1].length === 40) return m[1];
  const bits = [...m[1].toUpperCase()].map((c) => 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567'.indexOf(c).toString(2).padStart(5, '0')).join('');
  return bits.match(/.{4}/g).map((b) => parseInt(b, 2).toString(16)).join('');
}

function renderResults() {
  const { results, errors } = state;
  $('toolbar').hidden = !results.length;
  $('genre-pill').hidden = !state.genre;
  $('genre-pill').innerHTML = state.genre ? `${esc(genreLabel(state.genre))} ${ICONS.x}` : '';
  $('genre-pill').title = 'Tür seçimini kaldır';
  $('notice').hidden = true;
  if (errors?.length) {
    showNotice(`Bazı kaynaklara ulaşılamadı: ${errors.map((e) => `<strong>${esc(e.provider)}</strong>`).join(', ')}.`);
  }
  if (!results.length) {
    $('results').innerHTML = '';
    if (!errors?.length) showNotice('<strong>Sonuç bulunamadı.</strong> Farklı bir kelime ya da başka bir kategori dene.');
    return;
  }
  renderGenreFilter();
  renderList();
}

function renderList() {
  const list = visibleResults();
  const trCount = list.filter((r) => r.turkish).length;
  $('results-summary').innerHTML = `<strong>${list.length}</strong> sonuç${trCount ? ` <span class="muted">· ${trCount} Türkçe</span>` : ''}`;
  $('results').innerHTML = list.map((r, i) => {
    const type = ICONS[r.category] ? r.category : 'movies';
    const genres = (r.genres || []).slice(0, 2).map(genreLabel).join(', ');
    const meta = [
      r.turkish ? '<span class="pill tr">Türkçe</span>' : '',
      r.adult ? '<span class="pill adult">+18</span>' : '',
      r.license ? `<span class="pill license" title="${r.license === 'CC' ? 'Creative Commons lisanslı' : 'Telif süresi dolmuş, kamu malı'}">${esc(r.license)}</span>` : '',
      r.year ? `<span>${r.year}</span>` : '',
      genres ? `<span>${esc(genres)}</span>` : '',
      r.size ? `<span>${formatBytes(r.size)}</span>` : '',
      healthPill(r),
      `<span class="source">${esc(r.source)}</span>`,
    ].filter(Boolean).join('<span class="dot"></span>');
    const title = r.pageUrl
      ? `<a href="${esc(r.pageUrl)}" target="_blank" rel="noopener">${esc(r.title)}</a>`
      : esc(r.title);
    const tile = r.poster
      ? `<img class="poster" src="${esc(r.poster)}" alt="" loading="lazy" referrerpolicy="no-referrer" />`
      : `<div class="type-tile ${type}">${ICONS[type]}</div>`;
    return `<li class="result" data-delay="${Math.min(i, 12) * 25}" data-id="${esc(r.id)}">
      ${tile}
      <div class="result-main">
        <h3 class="result-title" title="${esc(r.title)}">${title}</h3>
        <div class="result-meta">${meta}</div>
      </div>
      <div class="result-actions">${ratingBadge(r)}${resultButton(r)}</div>
    </li>`;
  }).join('') || '<li class="notice">Bu türde sonuç yok.</li>';
  applyInlineStyles($('results'));
  for (const img of $('results').querySelectorAll('img.poster')) {
    img.addEventListener('error', () => img.replaceWith(Object.assign(document.createElement('div'), { className: 'type-tile movies', innerHTML: ICONS.movies })), { once: true });
  }
}

// CSP satır içi style özniteliklerine izin vermez; değerler JS ile atanır.
function applyInlineStyles(root) {
  for (const el of root.querySelectorAll('[data-delay]')) el.style.animationDelay = `${el.dataset.delay}ms`;
  for (const el of root.querySelectorAll('[data-width]')) el.style.width = `${el.dataset.width}%`;
}

function refreshResultButtons() {
  for (const li of $('results').children) {
    const r = state.results.find((x) => x.id === li.dataset.id);
    const btn = li.querySelector('.result-actions > button');
    if (r && btn) btn.outerHTML = resultButton(r);
  }
}

$('results').addEventListener('click', (e) => {
  const dl = e.target.closest('[data-dl]');
  if (dl) {
    const r = state.results.find((x) => x.id === dl.dataset.dl);
    // Kütüphanede afiş, yıl ve tür görünsün diye sonuç bilgileri de gönderilir.
    if (r) {
      const { magnet, torrentUrl, title, source, year, poster, genres, imdbId, imdbUrl, pageUrl, license, category } = r;
      startDownload({ magnet, torrentUrl, title, source, year, poster, genres, imdbId, imdbUrl, pageUrl, license, category }, dl);
    }
  }
  const open = e.target.closest('[data-open]');
  if (open) api(`downloads/${open.dataset.open}/open`, { method: 'POST' }).catch((err) => toast(err.message, 'error'));
  const resume = e.target.closest('[data-resume]');
  if (resume) api(`downloads/${resume.dataset.resume}/resume`, { method: 'POST' }).then(refreshDownloads).catch((err) => toast(err.message, 'error'));
  const watch = e.target.closest('[data-watch]');
  if (watch) openPlayer(watch.dataset.watch);
  const external = e.target.closest('[data-external]');
  if (external) openMainExternally(external.dataset.external);
  const pick = e.target.closest('[data-pick]');
  if (pick) choosePickFor(pick.dataset.pick);
  if (e.target.closest('[data-show-downloads]')) openDrawer('active');
});

async function startDownload(body, button) {
  if (button) { button.disabled = true; button.textContent = 'Başlıyor…'; }
  try {
    // Dosya listesi bilinmiyor: tek dosyaysa sunucu doğrudan indirir,
    // çok dosyaysa "picking" döner ve seçim penceresi açılır.
    const d = await api('downloads', { method: 'POST', body: JSON.stringify(body) });
    if (d.status === 'picking') choosePickFor(d.id);
    else {
      if (d.status === 'connecting' && !d.fileCount) awaitingPick.add(d.id);
      toast(d.fileCount ? 'İndirme başladı' : 'Dosyalar bakılıyor…');
    }
    await refreshDownloads();
  } catch (err) {
    toast(err.message, 'error');
    if (button) { button.disabled = false; button.textContent = 'İndir'; }
  }
}

/* ---------- İndirmeler ---------- */

const STATUS = {
  connecting: 'Bağlanıyor…',
  picking: 'Dosya seçimi bekliyor',
  queued: 'Sırada',
  downloading: 'İndiriliyor',
  paused: 'Duraklatıldı',
  done: 'Tamamlandı',
  error: 'Hata',
};

state.tab = 'active';
const seenDone = new Set(); // bildirim bir kez gösterilsin
let firstLoad = true;

function formatDate(ts) {
  if (!ts) return '';
  return new Date(ts).toLocaleString('tr-TR', { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
}

function activeCard(d) {
  const pct = Math.floor(d.progress * 100);
  let right = '';
  if (d.status === 'downloading') {
    right = [`${formatBytes(d.downloadSpeed) || '0 B'}/sn`, d.timeRemaining != null ? `${formatTime(d.timeRemaining)} kaldı` : '', `${d.peers} bağlantı`].filter(Boolean).join(' · ');
  } else if (d.status === 'error') right = esc(d.error || '');
  else if (d.status === 'picking') right = `${d.fileCount} dosya`;
  if (d.skippedPrograms && d.status !== 'error') right = [right, `${d.skippedPrograms} program dosyası atlandı`].filter(Boolean).join(' · ');
  const toggle = d.status === 'picking'
    ? '<button class="primary-btn small" type="button" data-act="pick">Dosya seç</button>'
    : d.status === 'queued'
      ? `<button class="icon-btn" data-act="resume" aria-label="Şimdi başlat" title="Sırayı beklemeden başlat">${ICONS.play}</button>`
      : d.status === 'paused' || d.status === 'error'
      ? `<button class="icon-btn" data-act="resume" aria-label="Devam et" title="Devam et">${ICONS.play}</button>`
      : `<button class="icon-btn" data-act="pause" aria-label="Duraklat" title="Duraklat">${ICONS.pause}</button>`;
  const label = d.retry ? `Bağlantı koptu, yeniden deneniyor (${d.retry}/5)…`
    : d.status === 'connecting' && !d.fileCount && d.key.startsWith('magnet:') ? 'Dosyalar bakılıyor…' : STATUS[d.status] || d.status;
  // İndirme sürerken de izlenebilir: izlenen dosya öne alınır ve sırayla iner.
  const watch = d.videos?.some((v) => v.playableInBrowser) && d.status === 'downloading' && d.progress > 0.02
    ? `<button class="icon-btn" data-act="watch" aria-label="İndirirken izle" title="İndirirken izle">${ICONS.tv}</button>`
    : '';
  return `<li class="dl ${d.status}" data-id="${d.id}">
    <div class="dl-top">
      <p class="dl-title" title="${esc(d.title)}">${esc(d.title)}</p>
      <div class="dl-actions">
        ${watch}${toggle}
        <button class="icon-btn" data-act="open" aria-label="Klasörde göster" title="Klasörde göster">${ICONS.folder}</button>
        <button class="icon-btn" data-act="remove" aria-label="Listeden kaldır" title="Listeden kaldır (dosyalar silinmez)">${ICONS.trash}</button>
      </div>
    </div>
    <div class="bar"><span data-width="${pct}"></span></div>
    <div class="dl-stats"><span><span class="state">${label}</span>${!['connecting', 'picking', 'queued'].includes(d.status) ? ` · %${pct}` : ''}</span><span>${right}</span></div>
  </li>`;
}

// En büyük video asıl dosyadır (örnek/sample dosyalar küçüktür).
function mainVideo(d) {
  return d.videos?.length ? d.videos.reduce((a, b) => (b.length > a.length ? b : a)) : null;
}

function openMainExternally(id) {
  const d = state.downloads.find((x) => x.id === id);
  const v = d && mainVideo(d);
  const req = v
    ? api(`downloads/${id}/open-file`, { method: 'POST', body: JSON.stringify({ index: v.index }) })
    : Promise.reject(new Error('Video dosyası yok.'));
  req.catch(() => api(`downloads/${id}/open`, { method: 'POST' })).catch((err) => toast(err.message, 'error'));
}

function doneCard(d) {
  const inBrowser = d.videos?.some((v) => v.playableInBrowser);
  const canPlay = d.videos?.length && !d.missing;
  const tileAct = inBrowser ? 'watch' : 'external';
  const tileTitle = !canPlay ? (d.missing ? 'Dosyalar bulunamadı' : 'Video dosyası yok') : inBrowser ? 'Dahili oynatıcıda izle' : 'Harici oynatıcıda aç';
  const reason = canPlay && !inBrowser ? '<span class="reason">Bu biçim yerleşik oynatıcıda açılmıyor</span>' : '';
  const ratio = d.ratio ? `oran ${d.ratio.toLocaleString('tr-TR', { maximumFractionDigits: 2 })}` : '';
  const info = d.missing
    ? '<span class="missing">Dosyalar taşınmış ya da silinmiş</span>'
    : d.seeding
      ? ['<span class="seeding">Paylaşılıyor</span>', `↑ ${formatBytes(d.uploadSpeed) || '0 B'}/sn`, ratio].filter(Boolean).join(' · ')
      : [reason, formatBytes(d.length), d.videos?.length > 1 ? `${d.videos.length} video` : '', formatDate(d.completedAt), ratio].filter(Boolean).join(' · ');
  return `<li class="dl done-card${d.missing ? ' is-missing' : ''}" data-id="${d.id}">
    <button class="play-tile${canPlay && !inBrowser ? ' external' : ''}" type="button" data-act="${tileAct}" ${canPlay ? '' : 'disabled'} aria-label="${tileTitle}" title="${tileTitle}">${canPlay ? ICONS.play : ICONS.file}</button>
    <div class="dl-body">
      <p class="dl-title" title="${esc(d.title)}">${esc(d.title)}</p>
      <p class="dl-sub">${info}</p>
    </div>
    <div class="dl-actions">
      <button class="icon-btn" data-act="open" aria-label="Klasörde göster" title="Klasörde göster">${ICONS.folder}</button>
      <button class="icon-btn" data-act="remove" aria-label="Listeden kaldır" title="Listeden kaldır (dosyalar silinmez)">${ICONS.trash}</button>
    </div>
  </li>`;
}

function renderDownloads() {
  const done = state.downloads.filter((d) => d.status === 'done')
    .sort((a, b) => (b.completedAt || 0) - (a.completedAt || 0));
  // En çok inen en üstte; aynı yüzdedekiler eklenme sırasını korur.
  const active = state.downloads.filter((d) => d.status !== 'done').sort((a, b) => (b.progress || 0) - (a.progress || 0));
  const running = active.filter((d) => d.status === 'downloading' || d.status === 'connecting').length;
  $('downloads-badge').hidden = !running;
  $('downloads-badge').textContent = running;
  $('count-active').textContent = active.length;
  $('count-done').textContent = done.length;
  for (const t of document.querySelectorAll('.tabs [data-tab]')) t.setAttribute('aria-selected', String(t.dataset.tab === state.tab));

  const list = state.tab === 'done' ? done : active;
  $('downloads-empty').hidden = list.length > 0;
  $('downloads-empty-text').innerHTML = state.tab === 'done'
    ? 'Biten indirmeler burada görünür.<br />Videoları tek tıkla izleyebilirsin.'
    : 'Şu an süren indirme yok.<br />Bir sonuçta <strong>İndir</strong>’e basman yeterli.';
  const html = list.map(state.tab === 'done' ? doneCard : activeCard).join('');

  // Var olan kartları yerinde güncelle ki ilerleme çubuğu akıcı ilerlesin
  // ve imleç altındaki butonlar her saniye yeniden oluşmasın.
  const tpl = document.createElement('template');
  tpl.innerHTML = html;
  const container = $('downloads');
  const existing = new Map([...container.children].map((el) => [el.dataset.id, el]));
  const next = [...tpl.content.children].map((fresh) => {
    const old = existing.get(fresh.dataset.id);
    if (old && old.outerHTML === fresh.outerHTML) return old; // tamamlanan kartlar değişmediyse dokunma
    const sameShape = old && old.className === fresh.className && old.querySelector('.bar')
      && old.querySelector('.dl-actions').innerHTML === fresh.querySelector('.dl-actions').innerHTML;
    if (!sameShape) {
      applyInlineStyles(fresh);
      return fresh;
    }
    old.querySelector('.bar > span').style.width = `${fresh.querySelector('.bar > span').dataset.width}%`;
    old.querySelector('.dl-stats').replaceWith(fresh.querySelector('.dl-stats'));
    return old;
  });
  container.replaceChildren(...next);
}

function notifyFinished(list) {
  for (const d of list) {
    if (d.status !== 'done' || seenDone.has(d.id)) continue;
    seenDone.add(d.id);
    if (firstLoad) continue;
    loadLibrary();
    toast(`İndirme tamamlandı: ${d.title}`);
    try {
      if ('Notification' in window && Notification.permission === 'granted') new Notification('İndirme tamamlandı', { body: d.title, icon: 'icon.svg' });
    } catch { /* bildirim desteklenmiyor */ }
  }
  firstLoad = false;
}

async function refreshDownloads() {
  try {
    const list = await api('downloads');
    state.downloads = list;
    notifyFinished(list);
    // Bu oturumda başlatılan magnet'in dosya listesi geldiyse seçimi sor.
    for (const d of list) {
      if (!awaitingPick.has(d.id) || d.status === 'connecting') continue;
      awaitingPick.delete(d.id);
      if (d.status === 'picking') choosePickFor(d.id);
    }
    renderDownloads();
    refreshResultButtons();
  } catch {
    // sunucu kapanıyor olabilir
  }
}

document.querySelector('.tabs').addEventListener('click', (e) => {
  const tab = e.target.closest('[data-tab]');
  if (!tab) return;
  state.tab = tab.dataset.tab;
  $('downloads').replaceChildren();
  renderDownloads();
});

$('downloads').addEventListener('click', async (e) => {
  const btn = e.target.closest('[data-act]');
  if (!btn) return;
  const id = btn.closest('[data-id]').dataset.id;
  const act = btn.dataset.act;
  if (act === 'watch') return openPlayer(id);
  if (act === 'external') return openMainExternally(id);
  if (act === 'pick') return choosePickFor(id);
  try {
    if (act === 'remove') await api(`downloads/${id}`, { method: 'DELETE' });
    else await api(`downloads/${id}/${act}`, { method: 'POST' });
    if (act === 'remove') toast('Listeden kaldırıldı. Dosyalar klasörde duruyor.');
    refreshDownloads();
  } catch (err) {
    toast(err.message, 'error');
  }
});

function openDrawer(tab) {
  if (tab) { state.tab = tab; $('downloads').replaceChildren(); renderDownloads(); }
  $('drawer').classList.add('open');
  $('drawer').setAttribute('aria-hidden', 'false');
  $('scrim').hidden = false;
  // Tamamlanınca masaüstü bildirimi için bir kez izin iste.
  try { if ('Notification' in window && Notification.permission === 'default') Notification.requestPermission(); } catch { /* yok */ }
}
function closeDrawer() {
  $('drawer').classList.remove('open');
  $('drawer').setAttribute('aria-hidden', 'true');
  $('scrim').hidden = true;
}
$('downloads-btn').addEventListener('click', () => openDrawer());
$('drawer-close').addEventListener('click', closeDrawer);
$('scrim').addEventListener('click', closeDrawer);
$('open-dir-btn').addEventListener('click', () => api('open-download-dir', { method: 'POST' }).catch((err) => toast(err.message, 'error')));
document.addEventListener('keydown', (e) => { if (e.key === 'Escape' && !$('player').open) closeDrawer(); });

/* ---------- .torrent dosyalarını toplu ekleme ---------- */

const imported = []; // { file, data, infoHash, name, length, fileCount, private, existing, error, checked }

function readAsBase64(file) {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result).split(',')[1] || '');
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

async function addTorrentFiles(fileList) {
  const files = [...fileList].filter((f) => /\.torrent$/i.test(f.name) && f.size < 10 * 1024 * 1024);
  if (!files.length) return toast('Seçilenlerde .torrent dosyası yok.', 'error');
  openImport();
  $('import-summary').textContent = `${files.length} dosya okunuyor…`;
  const payload = await Promise.all(files.map(async (f) => ({ name: f.name, data: await readAsBase64(f) })));
  const infos = await api('torrents/inspect', { method: 'POST', body: JSON.stringify({ files: payload }) });
  infos.forEach((info, i) => {
    if (info.infoHash && imported.some((x) => x.infoHash === info.infoHash)) return;
    // Film yerine yalnızca program içeren torrent eklenemez.
    if (info.blocked) info.error = 'Film değil: çalıştırılabilir dosya (.exe gibi) içeriyor. Güvenlik için eklenmez.';
    imported.push({ ...info, data: payload[i].data, checked: !info.error && !info.existing });
  });
  renderImport();
}

function renderImport() {
  const ok = imported.filter((x) => !x.error);
  const chosen = ok.filter((x) => x.checked);
  $('import-pick').hidden = imported.length > 0;
  $('import-list-wrap').hidden = imported.length === 0;
  $('import-summary').textContent = `${ok.length} torrent bulundu · ${chosen.length} seçili · ${formatBytes(chosen.reduce((n, x) => n + (x.length || 0), 0)) || '0 B'}`;
  $('import-all').checked = ok.length > 0 && chosen.length === ok.length;
  $('import-go').disabled = chosen.length === 0;
  $('import-go').textContent = chosen.length ? `${chosen.length} torrenti indir` : 'İndir';
  $('import-list').innerHTML = imported.map((x, i) => {
    const badges = [
      x.private ? '<span class="pill private">Özel</span>' : '',
      x.existing ? '<span class="pill muted-pill">Zaten listede</span>' : '',
    ].join('');
    const sub = x.error
      ? `<span class="import-error">${esc(x.error)}</span>`
      : [formatBytes(x.length), x.fileCount > 1 ? `${x.fileCount} dosya` : '', x.programs ? `${x.programs} program dosyası atlanacak` : ''].filter(Boolean).join(' · ');
    return `<li class="${x.error ? 'is-error' : ''}">
      <label class="check">
        <input type="checkbox" data-i="${i}" ${x.checked ? 'checked' : ''} ${x.error ? 'disabled' : ''} />
        <span class="import-main">
          <span class="import-name" title="${esc(x.file)}">${esc(x.name || x.file)}</span>
          <span class="import-sub">${sub}${badges}</span>
        </span>
      </label>
    </li>`;
  }).join('');
}

function openImport() {
  if (!$('import').open) $('import').showModal();
}

function resetImport() {
  imported.length = 0;
  renderImport();
}

$('import-btn').addEventListener('click', () => { resetImport(); openImport(); });
$('pick-files').addEventListener('click', () => $('import-files').click());
$('pick-more').addEventListener('click', () => $('import-files').click());
$('pick-folder').addEventListener('click', () => $('import-folder').click());
for (const id of ['import-files', 'import-folder']) {
  $(id).addEventListener('change', (e) => {
    addTorrentFiles(e.target.files).catch((err) => toast(err.message, 'error'));
    e.target.value = '';
  });
}
$('import-list').addEventListener('change', (e) => {
  const i = e.target.dataset.i;
  if (i == null) return;
  imported[i].checked = e.target.checked;
  renderImport();
});
$('import-all').addEventListener('change', () => {
  for (const x of imported) if (!x.error) x.checked = $('import-all').checked;
  renderImport();
});
$('import').addEventListener('close', async () => {
  const chosen = $('import').returnValue === 'go' ? imported.filter((x) => x.checked && !x.error) : [];
  resetImport();
  if (!chosen.length) return;
  let ok = 0;
  for (const x of chosen) {
    try {
      // Çok dosyalı torrentlerde sırayla hangi dosyaların ineceği sorulur; Vazgeç o torrenti atlar.
      let fileIndexes;
      if ((x.files || []).filter(pickable).length > 1 && !x.existing) {
        fileIndexes = await pickFiles(x.name || x.file, x.files || []);
        if (!fileIndexes) continue;
      }
      await api('downloads', { method: 'POST', body: JSON.stringify({ torrentFile: x.data, title: x.name, fileIndexes }) });
      ok++;
    } catch (err) {
      toast(`${x.name}: ${err.message}`, 'error');
    }
  }
  if (ok) toast(`${ok} torrent indirmeye eklendi`);
  await refreshDownloads();
  openDrawer('active');
});

// Pencerenin herhangi bir yerine .torrent dosyası sürükleyip bırakma.
let dragDepth = 0;
const hasFiles = (e) => [...(e.dataTransfer?.types || [])].includes('Files');
window.addEventListener('dragenter', (e) => { if (!hasFiles(e)) return; e.preventDefault(); dragDepth++; $('drop-overlay').hidden = false; });
window.addEventListener('dragover', (e) => { if (hasFiles(e)) e.preventDefault(); });
window.addEventListener('dragleave', () => { if (--dragDepth <= 0) { dragDepth = 0; $('drop-overlay').hidden = true; } });
window.addEventListener('drop', (e) => {
  if (!hasFiles(e)) return;
  e.preventDefault();
  dragDepth = 0;
  $('drop-overlay').hidden = true;
  addTorrentFiles(e.dataTransfer.files).catch((err) => toast(err.message, 'error'));
});

/* ---------- Dosya seçimi ---------- */

const awaitingPick = new Set(); // dosya listesi beklenen, bu oturumda başlatılan magnet'ler
const pickPending = new Set(); // penceresi açık ya da sırada olan indirmeler
const KIND_LABEL = { video: 'Video', subtitle: 'Altyazı', other: 'Diğer', program: 'Program · güvenlik için indirilmez' };
// Program dosyaları (.exe vb.) seçilemez; sunucu da reddeder.
const pickable = (f) => f.kind !== 'program';
const filePick = { files: [], selected: new Set() };
let pickChain = Promise.resolve();

// Varsayılan öneri: en büyük video ve ona uyan altyazı. Video yoksa en büyük dosya.
function defaultPicks(all) {
  const files = all.filter(pickable);
  const largest = (list) => list.reduce((a, b) => (b.length > a.length ? b : a));
  const picks = new Set();
  if (!files.length) return picks;
  const videos = files.filter((f) => f.kind === 'video');
  if (!videos.length) return picks.add(largest(files).index);
  const main = largest(videos.filter((v) => !/sample/i.test(v.name)).length ? videos.filter((v) => !/sample/i.test(v.name)) : videos);
  picks.add(main.index);
  const base = main.name.replace(/\.[^.]+$/, '').toLowerCase();
  const subs = files.filter((f) => f.kind === 'subtitle');
  const sub = subs.find((f) => f.name.toLowerCase().startsWith(base))
    || subs.find((f) => /(^|[^a-z0-9ğüşıöç])(tur|tr|türkçe|turkce)([^a-z0-9ğüşıöç]|$)/i.test(f.name));
  if (sub) picks.add(sub.index);
  return picks;
}

function renderFilePick() {
  const { files, selected } = filePick;
  const choosable = files.filter(pickable).length;
  const size = files.filter((f) => selected.has(f.index)).reduce((n, f) => n + f.length, 0);
  $('file-pick-summary').textContent = `${files.length} dosya · seçilenler ${selected.size} · ${formatBytes(size) || '0 B'}`;
  $('file-pick-all').checked = choosable > 0 && selected.size === choosable;
  $('file-pick-all').indeterminate = selected.size > 0 && selected.size < choosable;
  $('file-pick-go').disabled = selected.size === 0;
  $('file-pick-go').textContent = selected.size ? 'Seçilenleri indir' : 'İndir';
  for (const input of $('file-pick-list').querySelectorAll('[data-fi]')) input.checked = selected.has(Number(input.dataset.fi));
}

// Pencereyi açar, seçilen indeksleri ya da Vazgeç'te null döndürür. Pencereler
// sıraya girer; aynı anda iki kez showModal çağrılmaz.
function pickFiles(title, files) {
  const run = () => new Promise((resolve) => {
    filePick.files = files;
    filePick.selected = defaultPicks(files);
    $('file-pick-name').textContent = title || '';
    $('file-pick-name').title = title || '';
    $('file-pick-list').innerHTML = files.map((f) => `<li class="${pickable(f) ? '' : 'is-error'}">
      <label class="check">
        <input type="checkbox" data-fi="${f.index}" ${pickable(f) ? '' : 'disabled'} />
        <span class="import-main">
          <span class="import-name" title="${esc(f.path || f.name)}">${esc(f.name)}</span>
          <span class="import-sub">${KIND_LABEL[f.kind] || 'Diğer'} · ${formatBytes(f.length) || '0 B'}</span>
        </span>
      </label>
    </li>`).join('');
    renderFilePick();
    const dialog = $('file-pick');
    dialog.returnValue = '';
    dialog.addEventListener('close', () => {
      const chosen = [...filePick.selected].sort((a, b) => a - b);
      resolve(dialog.returnValue === 'go' && chosen.length ? chosen : null);
    }, { once: true });
    if (!dialog.open) dialog.showModal();
  });
  const result = pickChain.then(run);
  pickChain = result.catch(() => {});
  return result;
}

// "picking" durumundaki indirme için listeyi sunucudan alıp seçimi gönderir.
async function choosePickFor(id) {
  if (pickPending.has(id)) return;
  pickPending.add(id);
  try {
    const info = await api(`downloads/${id}/files`);
    if (!info.files.length) return toast('Dosya listesi henüz gelmedi.', 'error');
    const indexes = await pickFiles(info.title, info.files);
    if (!indexes) return; // Vazgeç: kayıt seçim bekler halde kalır
    await api(`downloads/${id}/files`, { method: 'POST', body: JSON.stringify({ indexes }) });
    toast('İndirme başladı');
    await refreshDownloads();
    openDrawer('active');
  } catch (err) {
    toast(err.message, 'error');
  } finally {
    pickPending.delete(id);
  }
}

$('file-pick-list').addEventListener('change', (e) => {
  const i = e.target.dataset.fi;
  if (i == null) return;
  if (e.target.checked) filePick.selected.add(Number(i));
  else filePick.selected.delete(Number(i));
  renderFilePick();
});
$('file-pick-all').addEventListener('change', () => {
  filePick.selected = new Set($('file-pick-all').checked ? filePick.files.filter(pickable).map((f) => f.index) : []);
  renderFilePick();
});

/* ---------- Oynatıcı ---------- */

const player = { id: null, index: null, infoHash: null, videos: [], lastSent: 0, localSub: null };
const RATES = ['0.75', '1', '1.25', '1.5', '2'];
const savedRate = store.get('playbackRate', '1');
$('player-rate').value = RATES.includes(savedRate) ? savedRate : '1';

function applyRate() {
  const video = $('player-video');
  video.defaultPlaybackRate = video.playbackRate = Number($('player-rate').value);
}

// Süre bilinmiyorsa (akış henüz başlarken) duration gelene kadar bekle.
function seekWhenReady(video, seconds) {
  const seek = () => {
    if (!Number.isFinite(video.duration)) return false;
    video.currentTime = Math.min(seconds, Math.max(0, video.duration - 1));
    return true;
  };
  video.addEventListener('loadedmetadata', () => {
    if (seek()) return;
    const onChange = () => { if (seek()) video.removeEventListener('durationchange', onChange); };
    video.addEventListener('durationchange', onChange);
  }, { once: true });
}

// Kütüphanede bu dosya için kayıtlı konum (izlendiyse ya da bitmek üzereyse baştan).
function savedPosition(infoHash, index) {
  const item = state.library.find((e) => e.infoHash === infoHash)?.items.find((i) => i.fileIndex === index);
  if (!item || item.watched || (item.durationSec && item.durationSec - item.positionSec < 20)) return 0;
  return item.positionSec || 0;
}

function mediaUrl(id, index, suffix = '') {
  return `/media/${encodeURIComponent(id)}/${index}${suffix}?t=${encodeURIComponent(TOKEN)}`;
}

function showPlayerError(title) {
  const video = $('player-video');
  video.pause();
  video.removeAttribute('src');
  video.hidden = true;
  $('player-error-title').textContent = title;
  $('player-error').hidden = false;
}

function loadVideo(id, index, startAt = 0) {
  const video = $('player-video');
  player.id = id;
  player.index = index;
  player.lastSent = Date.now();
  $('player-error').hidden = true;
  video.hidden = false;
  video.replaceChildren();
  const d = state.downloads.find((x) => x.id === id);
  // Yerleşik oynatıcının açamayacağı biçimler (mkv, avi, x265…) hiç denenmez.
  if (player.videos.find((v) => v.index === index)?.playableInBrowser === false) {
    return showPlayerError('Bu biçim yerleşik oynatıcıda açılmıyor.');
  }
  // Altyazı indirme sürerken de istenir; yoksa istek 404 döner, iz sessizce yok sayılır.
  const track = Object.assign(document.createElement('track'), { kind: 'subtitles', label: 'Altyazı', srclang: 'tr', src: mediaUrl(id, index, '/subtitle.vtt'), default: !player.localSub });
  video.append(track);
  if (player.localSub) video.append(localTrack());
  video.src = mediaUrl(id, index);
  applyRate();
  video.addEventListener('loadedmetadata', applyRate, { once: true });
  // Kaldığı yerden devam: süre bilinince konuma atla.
  if (startAt > 0) seekWhenReady(video, startAt);
  $('player-hint').hidden = !d || d.status === 'done';
  $('player-hint').textContent = 'İndirme sürüyor: henüz inmeyen kısımlara atlamak biraz bekletebilir.';
  video.play().catch(() => {});
}

// entry: kütüphane kaydı (indirme listeden kaldırılmış olabilir). index/startAt: kaldığı yer.
function openPlayer(id, { entry, index, startAt = 0 } = {}) {
  const d = state.downloads.find((x) => x.id === id);
  const list = d?.videos?.length ? d.videos
    : (entry?.items || []).filter((i) => !i.missing).map((i) => ({ index: i.fileIndex, name: i.name, playableInBrowser: i.playableInBrowser }));
  if (!list.length) return toast('Bu indirmede video dosyası yok.', 'error');
  const videos = [...list].sort((a, b) => a.name.localeCompare(b.name, 'tr', { numeric: true }));
  player.videos = videos;
  player.infoHash = d?.infoHash || entry?.infoHash || null;
  $('player-title').textContent = entry?.title || d.title;
  $('player-episode').innerHTML = videos.map((v) => `<option value="${v.index}">${esc(v.name)}</option>`).join('');
  $('player-episode-wrap').hidden = videos.length < 2;
  // Birden çok video varsa (dizi bölümleri) oynatılabilen ilki, tek videoda en büyüğü açılır.
  const first = videos.find((v) => v.index === index)
    || (videos.length > 1 ? videos.find((v) => v.playableInBrowser) || videos[0] : d ? mainVideo(d) : videos[0]);
  $('player-episode').value = String(first.index);
  if (!$('player').open) $('player').showModal();
  loadVideo(id, first.index, first.index === index ? startAt : savedPosition(player.infoHash, first.index));
}

// İzleme konumunu kütüphaneye yaz (en fazla 5 sn'de bir; kapanışta ve bitişte hemen).
function sendProgress(extra = {}, force = false) {
  const video = $('player-video');
  if (!player.infoHash || player.index == null || !state.library.some((e) => e.infoHash === player.infoHash)) return;
  if (!force && Date.now() - player.lastSent < 5000) return;
  if (!video.currentTime && !extra.watched) return;
  player.lastSent = Date.now();
  const body = { fileIndex: player.index, positionSec: video.currentTime, durationSec: Number.isFinite(video.duration) ? video.duration : undefined, ...extra };
  api(`library/${player.infoHash}/progress`, { method: 'PUT', body: JSON.stringify(body) }).catch(() => {});
}

function localTrack() {
  return Object.assign(document.createElement('track'), { kind: 'subtitles', label: player.localSub.name, srclang: 'tr', src: player.localSub.url, default: true });
}

// Yerel .srt/.vtt: sunucu yalnızca VTT'ye çevirir, dosya bu oturumda kalır.
async function loadLocalSubtitle(file) {
  if (file.size > 1024 * 1024) return toast('Altyazı dosyası 1 MB’tan büyük olamaz.', 'error');
  try {
    const { vtt } = await api('subtitles/preview', { method: 'POST', body: JSON.stringify({ name: file.name, data: await readAsBase64(file) }) });
    if (player.localSub) URL.revokeObjectURL(player.localSub.url);
    player.localSub = { name: file.name, url: URL.createObjectURL(new Blob([vtt], { type: 'text/vtt' })) };
    const video = $('player-video');
    for (const t of video.querySelectorAll('track')) t.default = false;
    for (const tt of video.textTracks) tt.mode = 'disabled';
    video.append(localTrack());
    const added = video.textTracks[video.textTracks.length - 1];
    if (added) added.mode = 'showing';
    toast('Altyazı yüklendi');
  } catch (err) {
    toast(err.message, 'error');
  }
}

function closePlayer() {
  const video = $('player-video');
  if (video.getAttribute('src')) sendProgress({}, true);
  if (player.localSub) { URL.revokeObjectURL(player.localSub.url); player.localSub = null; }
  video.pause();
  video.removeAttribute('src');
  video.replaceChildren();
  video.load();
  if ($('player').open) $('player').close();
  setTimeout(loadLibrary, 300);
}

function openExternal() {
  api(`downloads/${player.id}/open-file`, { method: 'POST', body: JSON.stringify({ index: player.index }) })
    .catch(() => api(`downloads/${player.id}/open`, { method: 'POST' }))
    .catch((err) => toast(err.message, 'error'));
}

$('player-episode').addEventListener('change', () => {
  // Bölüm değişirken eskisinin konumu yazılır, yenisi kaldığı yerden açılır.
  if ($('player-video').getAttribute('src')) sendProgress({}, true);
  const index = Number($('player-episode').value);
  loadVideo(player.id, index, savedPosition(player.infoHash, index));
});
$('player-rate').addEventListener('change', () => {
  store.set('playbackRate', $('player-rate').value);
  applyRate();
});
$('player-sub-btn').addEventListener('click', () => $('player-sub-file').click());
$('player-sub-file').addEventListener('change', (e) => {
  const [file] = e.target.files;
  e.target.value = '';
  if (file) loadLocalSubtitle(file);
});
$('player-video').addEventListener('error', () => {
  if (!$('player-video').getAttribute('src')) return;
  showPlayerError('Bu dosya dahili oynatıcıda açılamıyor.');
});
$('player-video').addEventListener('timeupdate', () => sendProgress());
$('player-video').addEventListener('ended', () => sendProgress({ watched: true }, true));
$('player-close').addEventListener('click', closePlayer);
$('player').addEventListener('close', closePlayer);
$('player-external').addEventListener('click', openExternal);
$('player-error-external').addEventListener('click', openExternal);

/* ---------- Kütüphanen ---------- */

state.library = [];

// Şeritte gösterilecek dosya: yarıda kalan, yoksa izlenmemiş ilk, yoksa ilk.
function currentItem(entry) {
  const items = entry.items.filter((i) => !i.missing);
  return items.find((i) => !i.watched && i.positionSec > 0) || items.find((i) => !i.watched) || items[0] || entry.items[0];
}

function libraryCard(e) {
  const item = currentItem(e);
  const pct = item?.durationSec ? Math.min(100, Math.round((item.positionSec / item.durationSec) * 100)) : 0;
  const watched = Boolean(item?.watched);
  const missing = e.missing || !item;
  const label = missing ? 'Dosya bulunamadı' : pct > 0 && !watched ? 'Kaldığın yerden izle' : 'İzle';
  const thumb = e.poster
    ? `<img class="lib-thumb" src="${esc(e.poster)}" alt="" loading="lazy" referrerpolicy="no-referrer" />`
    : '';
  return `<li class="lib-row${missing ? ' is-missing' : ''}" data-hash="${esc(e.infoHash)}">
    ${thumb}
    <button class="lib-play" type="button" data-lib-play ${missing ? 'disabled' : ''} aria-label="${esc(e.title)}: ${label}" title="${label}">${esc(e.title)}</button>
    <span class="lib-year">${missing ? '<span class="missing">dosya yok</span>' : esc(e.year || '')}</span>
    ${pct > 0 && !watched && !missing ? `<span class="lib-pct">%${pct}</span>` : '<span class="lib-pct"></span>'}
    ${item && !missing ? `<button class="lib-mark" type="button" data-lib-watched aria-pressed="${watched}" title="${watched ? 'İzlenmedi olarak işaretle' : 'İzlendi olarak işaretle'}">${watched ? 'İzlendi' : 'İzlemedim'}</button>` : '<span></span>'}
  </li>`;
}

async function loadLibrary() {
  try {
    state.library = await api('library');
  } catch {
    return;
  }
  $('library').hidden = state.library.length === 0;
  $('library-list').innerHTML = state.library.map(libraryCard).join('');
  applyInlineStyles($('library-list'));
  for (const img of $('library-list').querySelectorAll('img')) {
    img.addEventListener('error', () => img.remove(), { once: true });
  }
}

$('library-list').addEventListener('click', (ev) => {
  const card = ev.target.closest('[data-hash]');
  const entry = card && state.library.find((x) => x.infoHash === card.dataset.hash);
  if (!entry) return;
  const item = currentItem(entry);
  if (ev.target.closest('[data-lib-watched]')) {
    api(`library/${entry.infoHash}/progress`, { method: 'PUT', body: JSON.stringify({ fileIndex: item.fileIndex, watched: !item.watched }) })
      .then(loadLibrary).catch((err) => toast(err.message, 'error'));
    return;
  }
  if (!ev.target.closest('[data-lib-play]') || !item || item.missing) return;
  const d = state.downloads.find((x) => x.id === item.downloadId && x.status === 'done');
  // Bitmeye 20 sn'den az kaldıysa baştan başlar.
  const nearEnd = item.durationSec && item.durationSec - item.positionSec < 20;
  const startAt = item.watched || nearEnd ? 0 : item.positionSec;
  if (!item.playableInBrowser) {
    if (d) return openMainExternally(d.id);
    return toast('Bu biçim yerleşik oynatıcıda açılmıyor.', 'error');
  }
  openPlayer(d ? d.id : `lib-${entry.infoHash}`, { entry, index: item.fileIndex, startAt });
});

/* ---------- Ayarlar ---------- */

function addTorznabRow(t = {}) {
  const row = $('torznab-row').content.firstElementChild.cloneNode(true);
  for (const input of row.querySelectorAll('[data-k]')) input.value = t[input.dataset.k] || '';
  row.querySelector('[data-remove]').addEventListener('click', () => row.remove());
  $('torznab-list').append(row);
}

$('settings-btn').addEventListener('click', async () => {
  const s = await api('settings');
  $('set-dir').value = s.downloadDir;
  $('set-archive').checked = s.archiveEnabled !== false;
  $('set-omdb').value = s.omdbApiKey || '';
  $('set-seed').checked = s.seedAfterDone === true;
  $('set-tray').checked = s.closeToTray === true;
  $('set-max').value = s.maxActiveDownloads;
  $('set-down').value = s.downloadLimitKbps;
  $('set-up').value = s.uploadLimitKbps;
  $('torznab-list').innerHTML = '';
  s.torznab.forEach(addTorznabRow);
  $('settings').showModal();
});
// Yerel Jackett'ın "tüm indeksleyiciler" adresi hazır gelir; kullanıcı yalnızca API anahtarını yapıştırır.
const JACKETT_DEFAULT = { name: 'Jackett', url: 'http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab' };
$('add-torznab').addEventListener('click', () => {
  addTorznabRow($('torznab-list').children.length ? {} : JACKETT_DEFAULT);
  $('torznab-list').lastElementChild.querySelector('[data-k="apiKey"]').focus();
});

$('settings').addEventListener('close', async () => {
  if ($('settings').returnValue !== 'save') return;
  const torznab = [...$('torznab-list').children].map((row) => {
    const t = {};
    for (const input of row.querySelectorAll('[data-k]')) t[input.dataset.k] = input.value.trim();
    return t;
  }).filter((t) => t.url);
  try {
    await api('settings', {
      method: 'PUT',
      body: JSON.stringify({ downloadDir: $('set-dir').value, archiveEnabled: $('set-archive').checked, omdbApiKey: $('set-omdb').value, seedAfterDone: $('set-seed').checked, closeToTray: $('set-tray').checked, torznab,
        maxActiveDownloads: Number($('set-max').value) || 1, downloadLimitKbps: Number($('set-down').value) || 0, uploadLimitKbps: Number($('set-up').value) || 0 }),
    });
    toast('Ayarlar kaydedildi');
    loadMeta().catch(() => {});
  } catch (err) {
    toast(err.message, 'error');
  }
});

window.addEventListener('scroll', () => {
  document.querySelector('.topbar').classList.toggle('scrolled', window.scrollY > 4);
}, { passive: true });

/* ---------- Başlat ---------- */

loadMeta().catch((err) => toast(err.message, 'error'));
refreshDownloads();
setInterval(refreshDownloads, 1000);
loadLibrary();
$('q').focus();
