//! IMDb rating, genre and poster for film and series results. With an OMDb key
//! in Settings OMDb is used; without one, Cinemeta (Stremio's open film/series
//! info service, IMDb data) which needs no key.

use crate::jsonfile::{now_ms, read_json_safe, write_json_atomic};
use crate::model::{Rating, SearchResult};
use crate::net::Net;
use crate::titles::parse_title;
use futures::future::{BoxFuture, FutureExt, Shared};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

const MAX_LOOKUPS: usize = 20;
/// A search waits at most this long; late answers go to the cache and are ready next time.
pub const BUDGET: Duration = Duration::from_millis(2500);
const MAX_CACHED: usize = 500;
const FOUND_TTL: i64 = 30 * 24 * 3600 * 1000; // found titles: 30 days
const MISS_TTL: i64 = 6 * 3600 * 1000; // "not found": 6 hours, so new releases can be found later

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Info {
    pub imdb_id: String,
    pub rating: Option<f64>,
    pub votes: Option<u64>,
    #[serde(default)]
    pub genre: String,
    pub year: Option<i32>,
    pub poster: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    value: Option<Info>,
    at: i64,
}

/// Persistent cache of answers: after a restart the same titles are not asked
/// again, so the free daily OMDb quota is not wasted. Without a file it lives in memory only.
pub struct RatingsCache {
    file: Option<PathBuf>,
    entries: Mutex<(IndexMap<String, Entry>, bool)>,
}

impl RatingsCache {
    pub fn open(file: Option<PathBuf>) -> Self {
        let stored: IndexMap<String, Entry> = file
            .as_ref()
            .and_then(|f| read_json_safe(f))
            .and_then(|v| match v {
                Value::Object(m) => Some(m.into_iter().filter_map(|(k, v)| serde_json::from_value(v).ok().map(|e| (k, e))).collect()),
                _ => None,
            })
            .unwrap_or_default();
        RatingsCache { file, entries: Mutex::new((stored, false)) }
    }

    /// None: not cached or expired. Some(None): known as not found.
    pub fn get(&self, key: &str, now: i64) -> Option<Option<Info>> {
        let mut g = self.entries.lock().unwrap();
        let e = g.0.get(key)?.clone();
        let ttl = if e.value.is_some() { FOUND_TTL } else { MISS_TTL };
        if now - e.at > ttl {
            g.0.shift_remove(key);
            g.1 = true;
            return None;
        }
        Some(e.value)
    }

    pub fn set(&self, key: &str, value: Option<Info>, now: i64) {
        let mut g = self.entries.lock().unwrap();
        g.0.shift_remove(key);
        g.0.insert(key.to_string(), Entry { value, at: now });
        // Oldest entries go first (insertion order is kept).
        while g.0.len() > MAX_CACHED {
            g.0.shift_remove_index(0);
        }
        g.1 = true;
    }

    pub fn flush(&self) {
        let Some(file) = &self.file else { return };
        let mut g = self.entries.lock().unwrap();
        if !g.1 {
            return;
        }
        g.1 = false;
        if let Err(e) = write_json_atomic(file, &g.0) {
            eprintln!("[omdb] önbellek yazılamadı: {e}");
        }
    }
}

fn cache_key(r: &SearchResult) -> String {
    if let Some(id) = &r.imdb_id {
        return id.clone();
    }
    let p = parse_title(&r.title);
    let year = r.year.or(p.year).map(|y| y.to_string()).unwrap_or_default();
    format!("{}|{}|{}", p.name.to_lowercase(), year, p.series || r.category == "tv")
}

fn parse_int_prefix(v: Option<&Value>) -> Option<i32> {
    let s = match v? {
        Value::Number(n) => return n.as_f64().map(|f| f as i32),
        Value::String(s) => s.trim().to_string(),
        _ => return None,
    };
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn https(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).filter(|s| s.starts_with("https://")).map(String::from)
}

type LookupResult = Result<Option<Info>, String>;

async fn fetch_omdb(net: &Net, api_key: &str, r: &SearchResult) -> LookupResult {
    let p = parse_title(&r.title);
    let mut url = url::Url::parse(&net.omdb).map_err(|e| e.to_string())?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("apikey", api_key);
        if let Some(id) = &r.imdb_id {
            q.append_pair("i", id);
        } else {
            q.append_pair("t", &p.name);
            if let Some(y) = r.year.or(p.year) {
                q.append_pair("y", &y.to_string());
            }
            q.append_pair("type", if p.series || r.category == "tv" { "series" } else { "movie" });
        }
    }
    let res = net.client.get(url).timeout(Duration::from_secs(5)).send().await.map_err(|e| e.to_string())?;
    if !res.status().is_success() {
        return Err(format!("OMDb {}", res.status().as_u16())); // temporary error: not cached
    }
    let d: Value = res.json().await.map_err(|e| e.to_string())?;
    if d.get("Response").and_then(Value::as_str) != Some("True") {
        return Ok(None);
    }
    let s = |k: &str| d.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let genre = s("Genre");
    Ok(Some(Info {
        imdb_id: s("imdbID"),
        rating: s("imdbRating").parse().ok().filter(|n: &f64| *n != 0.0),
        votes: s("imdbVotes").replace(',', "").parse().ok().filter(|n: &u64| *n != 0),
        genre: if genre == "N/A" { String::new() } else { genre },
        year: parse_int_prefix(d.get("Year")),
        poster: https(d.get("Poster")),
    }))
}

static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-z0-9]+").unwrap());
static NAME_SPLIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*[/|:]\s*").unwrap());

pub fn norm(s: &str) -> String {
    let lowered: String = s.to_lowercase().nfkd().filter(|c| !('\u{300}'..='\u{36f}').contains(c)).collect();
    NON_ALNUM.replace_all(&lowered.replace('&', "and"), " ").trim().to_string()
}

fn start_year(m: &Value) -> Option<i32> {
    let v = m.get("releaseInfo").filter(|v| !v.is_null() && v.as_str() != Some("")).or_else(|| m.get("year"));
    parse_int_prefix(v)
}

fn pick<'a>(metas: &'a [Value], n: &str, year: Option<i32>) -> Option<&'a Value> {
    let name = |m: &Value| norm(m.get("name").and_then(Value::as_str).unwrap_or_default());
    let nn = norm(n);
    let same: Vec<&Value> = metas.iter().filter(|m| name(m) == nn).collect();
    // When the year matches the name may match partly: "Dune Çöl Gezegeni" ↔ "Dune".
    let partial = |m: &Value| name(m).contains(&nn) || format!(" {nn} ").contains(&format!(" {} ", name(m)));
    match year {
        Some(y) => same
            .iter()
            .find(|m| (start_year(m).unwrap_or(0) - y).abs() <= 1)
            .copied()
            .or_else(|| metas.iter().find(|m| start_year(m) == Some(y) && partial(m))),
        None => same.first().copied(),
    }
}

async fn fetch_cinemeta(net: &Net, r: &SearchResult) -> LookupResult {
    let p = parse_title(&r.title);
    let year = r.year.or(p.year);
    let kind = if p.series || r.category == "tv" { "series" } else { "movie" };
    let get = |path: String| async move {
        let res = net.client.get(format!("{}/{}", net.cinemeta, path)).timeout(Duration::from_secs(5)).send().await.map_err(|e| e.to_string())?;
        if !res.status().is_success() {
            return Err(format!("Cinemeta {}", res.status().as_u16()));
        }
        res.json::<Value>().await.map_err(|e| e.to_string())
    };
    let mut meta: Option<Value> = None;
    if let Some(id) = &r.imdb_id {
        meta = get(format!("meta/{kind}/{id}.json")).await?.get("meta").cloned();
    } else {
        // Turkish sites often write "Turkish Name / Original Name" or "Name: Subtitle".
        let mut names: Vec<String> = vec![];
        for n in std::iter::once(p.name.as_str()).chain(NAME_SPLIT.split(&p.name)) {
            let n = n.trim().to_string();
            if n.chars().count() > 1 && !names.contains(&n) {
                names.push(n);
            }
        }
        for n in names.iter().take(3) {
            let enc = percent_encoding::utf8_percent_encode(n, percent_encoding::NON_ALPHANUMERIC).to_string();
            let data = get(format!("catalog/{kind}/top/search={enc}.json")).await?;
            let metas = data.get("metas").and_then(Value::as_array).cloned().unwrap_or_default();
            if let Some(m) = pick(&metas, n, year) {
                meta = Some(m.clone());
                break;
            }
        }
        if let Some(m) = &meta {
            let has_rating = m.get("imdbRating").is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
            if !has_rating {
                let id = m.get("id").or_else(|| m.get("imdb_id")).and_then(Value::as_str).unwrap_or_default().to_string();
                if let Some(full) = get(format!("meta/{kind}/{id}.json")).await?.get("meta").cloned().filter(|v| !v.is_null()) {
                    meta = Some(full);
                }
            }
        }
    }
    let Some(m) = meta.filter(|m| !m.is_null()) else { return Ok(None) };
    let imdb_id = m.get("imdb_id").and_then(Value::as_str).filter(|s| !s.is_empty()).or_else(|| m.get("id").and_then(Value::as_str)).unwrap_or_default();
    if !Regex::new(r"^tt\d+$").unwrap().is_match(imdb_id) {
        return Ok(None);
    }
    let join = |v: Option<&Value>| v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
    let rating = match m.get("imdbRating") {
        Some(Value::String(s)) => s.parse().ok(),
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    };
    Ok(Some(Info {
        imdb_id: imdb_id.to_string(),
        rating: rating.filter(|n| *n != 0.0),
        votes: None,
        genre: join(m.get("genres")).or_else(|| join(m.get("genre"))).unwrap_or_default(),
        year: start_year(&m),
        poster: https(m.get("poster")),
    }))
}

static WARNED_AT: AtomicI64 = AtomicI64::new(0);

fn warn_once(err: &str) {
    let now = now_ms();
    if now - WARNED_AT.load(Ordering::Relaxed) < 60_000 {
        return;
    }
    WARNED_AT.store(now, Ordering::Relaxed);
    eprintln!("[afiş] film bilgisi alınamadı: {err}");
}

type Pending = Shared<BoxFuture<'static, Option<Info>>>;
static INFLIGHT: Lazy<Mutex<HashMap<String, Pending>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn lookup(net: Net, api_key: String, r: SearchResult, cache: Arc<RatingsCache>, key: String) -> Pending {
    let mut map = INFLIGHT.lock().unwrap();
    if let Some(p) = map.get(&key) {
        return p.clone();
    }
    let k = key.clone();
    let fut = async move {
        let out = if api_key.is_empty() { fetch_cinemeta(&net, &r).await } else { fetch_omdb(&net, &api_key, &r).await };
        let value = match out {
            // Each answer is written to disk at once, so a search that is cut short loses nothing.
            Ok(v) => {
                cache.set(&k, v.clone(), now_ms());
                cache.flush();
                v
            }
            Err(e) => {
                warn_once(&e);
                None
            }
        };
        INFLIGHT.lock().unwrap().remove(&k);
        value
    }
    .boxed()
    .shared();
    map.insert(key, fut.clone());
    // Keep going after the search stops waiting, so late answers reach the cache.
    tokio::spawn(fut.clone());
    fut
}

pub async fn add_imdb_ratings(results: &mut [SearchResult], api_key: &str, net: &Net, cache: &Arc<RatingsCache>, budget: Duration) {
    // Cached answers don't count against the quota; only real lookups are capped at 20.
    // The key-less source has its own cache, so entering an OMDb key asks again.
    let prefix = if api_key.is_empty() { "cm2:" } else { "" };
    let targets: Vec<usize> = (0..results.len()).filter(|&i| results[i].category == "movies" || results[i].category == "tv").collect();
    let found: Arc<Mutex<Vec<Option<Info>>>> = Arc::new(Mutex::new(vec![None; targets.len()]));
    let mut pending = vec![];
    let mut lookups = 0;
    let now = now_ms();
    for (slot, &i) in targets.iter().enumerate() {
        let key = format!("{prefix}{}", cache_key(&results[i]));
        if let Some(cached) = cache.get(&key, now) {
            found.lock().unwrap()[slot] = cached;
            continue;
        }
        if lookups >= MAX_LOOKUPS {
            continue;
        }
        lookups += 1;
        let p = lookup(net.clone(), api_key.to_string(), results[i].clone(), cache.clone(), key);
        let found = found.clone();
        pending.push(async move {
            let v = p.await;
            found.lock().unwrap()[slot] = v;
        });
    }
    let _ = tokio::time::timeout(budget, futures::future::join_all(pending)).await;
    cache.flush();
    let found = found.lock().unwrap().clone();
    for (slot, &i) in targets.iter().enumerate() {
        let Some(info) = &found[slot] else { continue };
        let r = &mut results[i];
        if let Some(v) = info.rating {
            r.rating = Some(Rating { value: v, source: "IMDb".into(), votes: info.votes });
        }
        if !info.genre.is_empty() {
            r.subjects.extend(Regex::new(r",\s*").unwrap().split(&info.genre).map(String::from));
        }
        if r.year.is_none() {
            r.year = info.year;
        }
        if let Some(p) = &info.poster {
            r.poster = Some(p.clone());
        }
        r.imdb_url = Some(format!("https://www.imdb.com/title/{}/", info.imdb_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::testserver::{self, json, param};
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    fn film(title: &str) -> SearchResult {
        SearchResult { id: "1".into(), title: title.into(), category: "movies".into(), torrent_url: Some("a".into()), ..Default::default() }
    }

    #[tokio::test]
    async fn omdb_key_adds_rating_genre_and_poster() {
        let server = testserver::start(|_| {
            json(json!({"Response": "True", "imdbID": "tt0013442", "imdbRating": "7.8", "imdbVotes": "107,000", "Genre": "Fantasy, Horror", "Year": "1922", "Poster": "https://img.example/p.jpg"}))
        })
        .await;
        let cache = Arc::new(RatingsCache::open(None));
        let mut results = vec![film("Nosferatu.1922.1080p")];
        add_imdb_ratings(&mut results, "k", &Net::local(&server.base), &cache, BUDGET).await;
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(param(&seen[0], "t").as_deref(), Some("Nosferatu"));
        assert_eq!(param(&seen[0], "y").as_deref(), Some("1922"));
        let r = &results[0];
        assert_eq!(r.rating, Some(Rating { value: 7.8, source: "IMDb".into(), votes: Some(107000) }));
        let mut genres = crate::categories::detect_genres(&r.subjects);
        genres.sort();
        assert_eq!(genres, ["fantastik", "korku"]);
        assert_eq!(r.poster.as_deref(), Some("https://img.example/p.jpg"));
        assert_eq!(r.imdb_url.as_deref(), Some("https://www.imdb.com/title/tt0013442/"));
    }

    #[tokio::test]
    async fn cache_is_used_after_restart_so_the_same_title_is_not_asked_twice() {
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let server = testserver::start(move |_| {
            c.fetch_add(1, Ordering::SeqCst);
            json(json!({"Response": "True", "imdbID": "tt0000001", "imdbRating": "6.0", "Genre": "Drama"}))
        })
        .await;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("omdb-cache.json");
        let mut a = vec![film("Cache Test Film 1931")];
        add_imdb_ratings(&mut a, "k", &Net::local(&server.base), &Arc::new(RatingsCache::open(Some(file.clone()))), BUDGET).await;
        let mut b = vec![film("Cache Test Film 1931")];
        add_imdb_ratings(&mut b, "k", &Net::local(&server.base), &Arc::new(RatingsCache::open(Some(file))), BUDGET).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(b[0].rating.as_ref().unwrap().value, 6.0);
    }

    #[tokio::test]
    async fn without_key_cinemeta_gives_poster_and_rating_when_name_and_year_match() {
        let server = testserver::start(|req| {
            let path = req.uri().path().to_string();
            if path.contains("/catalog/movie/top/search=") {
                json(json!({"metas": [
                    {"id": "tt9999999", "name": "Dune", "releaseInfo": "1984", "imdbRating": "6.3"},
                    {"id": "tt1160419", "name": "Dune", "releaseInfo": "2021", "imdbRating": "8.0", "genres": ["Sci-Fi", "Adventure"], "poster": "https://img/dune.jpg"}
                ]}))
            } else {
                json(json!({"meta": null}))
            }
        })
        .await;
        let cache = Arc::new(RatingsCache::open(None));
        let mut results = vec![film("Dune.2021.1080p.WEB-DL"), film("Unknown Thing 2019")];
        add_imdb_ratings(&mut results, "", &Net::local(&server.base), &cache, BUDGET).await;
        assert_eq!(results[0].rating.as_ref().unwrap().value, 8.0);
        assert_eq!(results[0].poster.as_deref(), Some("https://img/dune.jpg"));
        assert_eq!(results[0].imdb_url.as_deref(), Some("https://www.imdb.com/title/tt1160419/"));
        assert!(results[1].imdb_url.is_none(), "different name is not matched");
    }

    #[tokio::test]
    async fn slow_service_does_not_hold_the_search_and_late_answer_is_ready_next_time() {
        let server = testserver::start(|_| json(json!({"Response": "True", "imdbID": "tt0000002", "imdbRating": "5.5"}))).await;
        // A server that answers late: wrap the address behind a slow proxy route.
        let slow = testserver::start(|_| {
            std::thread::sleep(Duration::from_millis(300));
            json(json!({"Response": "True", "imdbID": "tt0000002", "imdbRating": "5.5"}))
        })
        .await;
        let _ = server;
        let cache = Arc::new(RatingsCache::open(None));
        let mut first = vec![film("Slow Film 1950")];
        add_imdb_ratings(&mut first, "k", &Net::local(&slow.base), &cache, Duration::from_millis(50)).await;
        assert!(first[0].rating.is_none(), "search does not wait");
        tokio::time::sleep(Duration::from_millis(600)).await;
        let mut again = vec![film("Slow Film 1950")];
        add_imdb_ratings(&mut again, "k", &Net::local(&slow.base), &cache, Duration::from_millis(50)).await;
        assert_eq!(again[0].rating.as_ref().unwrap().value, 5.5);
    }

    #[test]
    fn norm_strips_accents_and_symbols() {
        assert_eq!(norm("Amélie & Co."), "amelie and co");
    }
}
