//! Torznab: indexers the user runs themselves (Jackett / Prowlarr). The app
//! ships no sites; the user enters the address and the API key.

use crate::categories::{category, category_from_torznab, genre_terms};
use crate::model::SearchResult;
use crate::net::{Net, SourceError};
use crate::search::SearchParams;
use crate::settings::Torznab;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::time::Duration;

static CDATA: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<!\[CDATA\[(.*?)\]\]>").unwrap());
static ITEM: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<item>(.*?)</item>").unwrap());
static ATTR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"(?i)<(?:torznab|newznab):attr\s+name="([^"]+)"\s+value="([^"]*)""#).unwrap());
static ENCLOSURE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)<enclosure[^>]*url="([^"]+)""#).unwrap());
static API_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"/api$").unwrap());
static TRAILING: Lazy<Regex> = Lazy::new(|| Regex::new(r"/+$").unwrap());

fn decode(s: &str) -> String {
    CDATA
        .replace_all(s, "$1")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
        .trim()
        .to_string()
}

fn tag(xml: &str, name: &str) -> String {
    let re = Regex::new(&format!(r"(?is)<{0}(?:\s[^>]*)?>(.*?)</{0}>", regex::escape(name))).unwrap();
    re.captures(xml).map(|c| decode(&c[1])).unwrap_or_default()
}

fn attrs(xml: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for c in ATTR.captures_iter(xml) {
        let key = c[1].to_lowercase();
        // Jackett writes the site's own code (100000+) after the standard category; keep the standard one.
        if key == "category" {
            if let Some(prev) = out.get("category") {
                if prev.parse::<f64>().map(|n| n < 100000.0).unwrap_or(false) {
                    continue;
                }
            }
        }
        out.insert(key, decode(&c[2]));
    }
    out
}

fn imdb(v: &str) -> String {
    let digits = v.trim_start_matches("tt");
    format!("tt{digits:0>7}")
}

fn positive(s: Option<&String>) -> Option<u64> {
    s.and_then(|v| v.trim().parse::<f64>().ok()).filter(|n| *n > 0.0).map(|n| n as u64)
}

pub fn parse_torznab(xml: &str, source_name: &str) -> Vec<SearchResult> {
    let items: Vec<&str> = ITEM.captures_iter(xml).map(|c| c.get(1).unwrap().as_str()).collect();
    let total = items.len();
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let a = attrs(item);
            let link = {
                let l = tag(item, "link");
                if !l.is_empty() {
                    l
                } else {
                    ENCLOSURE.captures(item).map(|c| decode(&c[1])).unwrap_or_default()
                }
            };
            let magnet = a.get("magneturl").cloned().filter(|s| !s.is_empty()).or_else(|| link.starts_with("magnet:").then(|| link.clone()));
            let size = positive(a.get("size")).or_else(|| positive(Some(&tag(item, "size"))));
            let cat = positive(a.get("category")).or_else(|| positive(Some(&tag(item, "category")))).map(|n| n as u32);
            let info_hash = a.get("infohash").map(|h| h.to_lowercase());
            let jackett = tag(item, "jackettindexer");
            let comments = tag(item, "comments");
            let id_part = a.get("infohash").cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| if link.is_empty() { index.to_string() } else { link.clone() });
            SearchResult {
                id: format!("torznab:{id_part}"),
                source: if jackett.is_empty() { source_name.to_string() } else { jackett },
                title: tag(item, "title"),
                year: positive(a.get("year")).map(|n| n as i32),
                language: a.get("language").filter(|s| !s.is_empty()).map(|l| vec![l.clone()]).unwrap_or_default(),
                category: category_from_torznab(cat).into(),
                torznab_category: cat,
                subjects: a.get("genre").filter(|s| !s.is_empty()).map(|g| Regex::new(r",\s*").unwrap().split(g).map(String::from).collect()).unwrap_or_default(),
                imdb_id: a.get("imdbid").or_else(|| a.get("imdb")).filter(|s| !s.is_empty()).map(|v| imdb(v)),
                size,
                seeders: a.get("seeders").and_then(|v| v.parse::<f64>().ok()).map(|n| n as u64),
                peers: a.get("peers").and_then(|v| v.parse::<f64>().ok()).map(|n| n as u64),
                info_hash,
                torrent_url: if magnet.is_some() || link.is_empty() { None } else { Some(link.clone()) },
                magnet,
                page_url: (!comments.is_empty()).then_some(comments),
                // Some indexers (Jackett) pass on the site's poster; an IMDb poster replaces it when found.
                poster: a.get("coverurl").filter(|u| u.starts_with("https://")).cloned(),
                relevance: ((total - index) as f64 / total.max(1) as f64) * 2.0,
                ..Default::default()
            }
        })
        .filter(|r| !r.title.is_empty() && (r.magnet.is_some() || r.torrent_url.is_some()))
        .collect()
}

pub fn endpoint(base: &str) -> String {
    let trimmed = TRAILING.replace(base, "");
    format!("{}{}", trimmed, if API_END.is_match(base) { "" } else { "/api" })
}

/// Time one indexer (or a plain Torznab address) has to answer.
pub const TIMEOUT: Duration = Duration::from_secs(20);
const LIST_TIMEOUT: Duration = Duration::from_secs(8);

static JACKETT_ALL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(.*/api/v2\.0/indexers/)all(/results/torznab)(?:/api)?/*$").unwrap());
static INDEXER: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?is)<indexer\s[^>]*\bid="([^"]+)"[^>]*>(.*?)</indexer>"#).unwrap());

/// Results of one source, plus the parts of it (single Jackett sites) that failed.
#[derive(Debug, Default)]
pub struct Found {
    pub results: Vec<SearchResult>,
    pub failures: Vec<(String, SourceError)>,
}

/// For Jackett's "all indexers" address, the address of one indexer.
fn jackett_indexer_url(all_url: &str, id: &str) -> Option<String> {
    JACKETT_ALL.captures(all_url.trim()).map(|c| format!("{}{}{}", &c[1], id, &c[2]))
}

/// Configured indexers as (id, title), read from Jackett's `t=indexers` answer.
pub fn parse_indexers(xml: &str) -> Vec<(String, String)> {
    INDEXER
        .captures_iter(xml)
        .map(|c| {
            let id = decode(&c[1]);
            let title = tag(&c[2], "title");
            (id.clone(), if title.is_empty() { id } else { title })
        })
        .collect()
}

fn with_key(url: &mut url::Url, t: &Torznab) {
    if !t.api_key.is_empty() {
        url.query_pairs_mut().append_pair("apikey", &t.api_key);
    }
}

async fn jackett_indexers(t: &Torznab, net: &Net) -> Result<Vec<(String, String)>, SourceError> {
    let mut url = url::Url::parse(&endpoint(&t.url)).map_err(|e| SourceError::msg(e.to_string()))?;
    url.query_pairs_mut().append_pair("t", "indexers").append_pair("configured", "true");
    with_key(&mut url, t);
    let res = net.client.get(url).timeout(LIST_TIMEOUT).send().await?;
    if !res.status().is_success() {
        return Err(SourceError::msg(format!("{}", res.status().as_u16())));
    }
    Ok(parse_indexers(&res.text().await?))
}

async fn run(base: &str, t: &Torznab, name: &str, net: &Net, q: &str, cat: &str, timeout: Duration) -> Result<Vec<SearchResult>, SourceError> {
    let mut url = url::Url::parse(&endpoint(base)).map_err(|e| SourceError::msg(format!("{name}: {e}")))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("t", "search");
        qp.append_pair("q", q);
        let cats = category(cat).torznab;
        if !cats.is_empty() {
            qp.append_pair("cat", &cats.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(","));
        }
    }
    with_key(&mut url, t);
    let res = net.client.get(url).timeout(timeout).send().await?;
    if !res.status().is_success() {
        return Err(SourceError::msg(format!("{name} {}", res.status().as_u16())));
    }
    Ok(parse_torznab(&res.text().await?, name))
}

/// The query, then the same query with "türkçe"; if only the Turkish one
/// fails, the main results still count.
async fn query_pair(base: &str, t: &Torznab, name: &str, net: &Net, terms: &str, cat: &str, timeout: Duration) -> Result<Vec<SearchResult>, SourceError> {
    let tr_terms = format!("{terms} türkçe");
    let (all, tr) = tokio::join!(run(base, t, name, net, terms, cat, timeout), run(base, t, name, net, &tr_terms, cat, timeout));
    let mut all = all?;
    all.extend(tr.unwrap_or_default());
    Ok(all)
}

fn dedupe(all: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    all.into_iter()
        .filter(|r| seen.insert(r.info_hash.clone().or_else(|| r.magnet.clone()).or_else(|| r.torrent_url.clone()).unwrap_or_default()))
        .collect()
}

pub async fn search(t: &Torznab, p: &SearchParams, net: &Net, timeout: Duration) -> Result<Found, SourceError> {
    let terms = [p.query.trim(), genre_terms(&p.genre).get(1).copied().unwrap_or("")]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if terms.is_empty() {
        return Ok(Found::default());
    }
    let name = if t.name.is_empty() { "Torznab" } else { &t.name };

    // Jackett's "all" address answers only when every site has answered, so a
    // single slow site used to lose every result. Each site is asked on its own
    // instead; a slow or broken one is reported and the rest still come back.
    if JACKETT_ALL.is_match(t.url.trim()) {
        if let Ok(indexers) = jackett_indexers(t, net).await {
            if !indexers.is_empty() {
                let terms = &terms;
                let runs = indexers.iter().map(|(id, title)| {
                    let base = jackett_indexer_url(&t.url, id).unwrap_or_default();
                    async move { query_pair(&base, t, title, net, terms, &p.category, timeout).await }
                });
                let settled = futures::future::join_all(runs).await;
                let mut found = Found::default();
                for ((_, title), r) in indexers.iter().zip(settled) {
                    match r {
                        Ok(v) => found.results.extend(v),
                        Err(e) => found.failures.push((format!("{name} · {title}"), e)),
                    }
                }
                if found.failures.len() == indexers.len() {
                    let timeout = found.failures.iter().all(|(_, e)| e.timeout);
                    let message = found.failures.into_iter().map(|(n, e)| format!("{n}: {}", e.message)).collect::<Vec<_>>().join("; ");
                    return Err(SourceError { timeout, message });
                }
                found.results = dedupe(found.results);
                return Ok(found);
            }
        }
        // The list could not be read (older Jackett, wrong key): ask "all", with more time.
        let results = query_pair(&t.url, t, name, net, &terms, &p.category, timeout * 3 / 2).await?;
        return Ok(Found { results: dedupe(results), failures: vec![] });
    }

    let results = query_pair(&t.url, t, name, net, &terms, &p.category, timeout).await?;
    Ok(Found { results: dedupe(results), failures: vec![] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::testserver::{self, param, text};
    use crate::ranking::rank_results;

    pub const SAMPLE: &str = r#"<?xml version="1.0"?><rss><channel>
<item><title>Big Buck Bunny 2008</title><link>http://jackett/dl/1.torrent</link><size>276134947</size>
<comments>http://example.org/1</comments>
<torznab:attr name="seeders" value="42"/><torznab:attr name="infohash" value="DD8255ECDC7CA55FB0BBF81323D87062DB1F6D1C"/>
<torznab:attr name="magneturl" value="magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&amp;dn=Big"/></item>
<item><title><![CDATA[Sintel TR Dublaj]]></title><link>http://jackett/dl/2.torrent</link>
<torznab:attr name="seeders" value="3"/></item>
</channel></rss>"#;

    #[test]
    fn torznab_xml_is_parsed() {
        let items = parse_torznab(SAMPLE, "Jackett");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].seeders, Some(42));
        assert_eq!(items[0].magnet.as_deref(), Some("magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c&dn=Big"));
        assert_eq!(items[0].info_hash.as_deref(), Some("dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c"));
        assert_eq!(items[0].torrent_url, None);
        assert_eq!(items[1].title, "Sintel TR Dublaj");
        assert_eq!(items[1].torrent_url.as_deref(), Some("http://jackett/dl/2.torrent"));
    }

    #[test]
    fn site_poster_is_used_and_standard_category_wins_over_site_code() {
        let xml = format!(
            r#"<rss><channel><item>
    <title>Dizi Adi S01E01 1080p</title>
    <link>magnet:?xt=urn:btih:{a}</link>
    <torznab:attr name="category" value="5000" />
    <torznab:attr name="category" value="100012" />
    <torznab:attr name="coverurl" value="https://site.example/poster.jpg" />
  </item><item>
    <title>Film 2020</title>
    <link>magnet:?xt=urn:btih:{b}</link>
    <torznab:attr name="coverurl" value="http://site.example/poster.jpg" />
  </item></channel></rss>"#,
            a = "a".repeat(40),
            b = "b".repeat(40)
        );
        let items = parse_torznab(&xml, "Jackett");
        assert_eq!(items[0].category, "tv");
        assert_eq!(items[0].poster.as_deref(), Some("https://site.example/poster.jpg"));
        assert_eq!(items[1].poster, None, "insecure (http) poster is not used");
    }

    fn jackett(server: &testserver::TestServer) -> Torznab {
        Torznab {
            id: "t".into(),
            name: "Jackett".into(),
            url: format!("{}/api/v2.0/indexers/all/results/torznab", server.base),
            api_key: "k".into(),
            enabled: true,
        }
    }

    fn bunny() -> SearchParams {
        SearchParams { query: "bunny".into(), category: "movies".into(), genre: String::new() }
    }

    const INDEXERS: &str = r#"<?xml version="1.0"?><indexers>
<indexer id="yts" configured="true"><title>YTS</title><caps/></indexer>
<indexer id="slow" configured="true"><title>Slow Site</title><caps/></indexer>
</indexers>"#;

    /// A response whose body never finishes, like an indexer that hangs.
    fn hang() -> axum::response::Response {
        let body = axum::body::Body::from_stream(futures::stream::pending::<Result<bytes::Bytes, std::io::Error>>());
        axum::response::Response::builder().body(body).unwrap()
    }

    #[test]
    fn jackett_indexer_address_and_list() {
        let all = "http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab/";
        assert_eq!(jackett_indexer_url(all, "yts").as_deref(), Some("http://127.0.0.1:9117/api/v2.0/indexers/yts/results/torznab"));
        assert_eq!(jackett_indexer_url("http://prowlarr/1/api", "yts"), None);
        assert_eq!(parse_indexers(INDEXERS), [("yts".to_string(), "YTS".to_string()), ("slow".to_string(), "Slow Site".to_string())]);
    }

    #[tokio::test]
    async fn jackett_is_asked_per_indexer_with_api_key_and_category() {
        let server = testserver::start(|req| if param(&req.uri().to_string(), "t").as_deref() == Some("indexers") { text(INDEXERS) } else { text(SAMPLE) }).await;
        let found = search(&jackett(&server), &bunny(), &Net::new(), TIMEOUT).await.unwrap();
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 5, "indexer list + 2 queries for each of 2 indexers");
        assert!(seen[0].starts_with("/api/v2.0/indexers/all/results/torznab/api?"));
        assert_eq!(param(&seen[0], "configured").as_deref(), Some("true"));
        let queries = &seen[1..];
        for id in ["yts", "slow"] {
            assert_eq!(queries.iter().filter(|u| u.starts_with(&format!("/api/v2.0/indexers/{id}/results/torznab/api?"))).count(), 2);
        }
        assert!(seen.iter().all(|u| param(u, "apikey").as_deref() == Some("k")));
        assert!(queries.iter().all(|u| param(u, "cat").as_deref() == Some("2000")));
        let mut qs: Vec<_> = queries.iter().map(|u| param(u, "q").unwrap()).collect();
        qs.sort();
        assert_eq!(qs, ["bunny", "bunny", "bunny türkçe", "bunny türkçe"]);
        assert!(found.failures.is_empty());
        assert_eq!(found.results.len(), 2, "same results from every query are kept once");
        assert!(rank_results(found.results).iter().find(|r| r.title == "Sintel TR Dublaj").unwrap().turkish);
    }

    #[tokio::test]
    async fn a_hanging_indexer_does_not_lose_the_others() {
        let server = testserver::start(|req| {
            let uri = req.uri().to_string();
            if param(&uri, "t").as_deref() == Some("indexers") {
                text(INDEXERS)
            } else if uri.contains("/indexers/slow/") {
                hang()
            } else {
                text(SAMPLE)
            }
        })
        .await;
        let found = search(&jackett(&server), &bunny(), &Net::new(), Duration::from_millis(500)).await.unwrap();
        assert_eq!(found.results.len(), 2);
        assert_eq!(found.failures.len(), 1);
        assert_eq!(found.failures[0].0, "Jackett · Slow Site");
        assert!(found.failures[0].1.timeout);
    }

    #[tokio::test]
    async fn every_indexer_timing_out_is_a_timeout() {
        let server = testserver::start(|req| if param(&req.uri().to_string(), "t").as_deref() == Some("indexers") { text(INDEXERS) } else { hang() }).await;
        let err = search(&jackett(&server), &bunny(), &Net::new(), Duration::from_millis(300)).await.unwrap_err();
        assert!(err.timeout);
    }

    #[tokio::test]
    async fn without_an_indexer_list_all_is_asked_with_more_time() {
        let server = testserver::start(|req| {
            if param(&req.uri().to_string(), "t").as_deref() == Some("indexers") {
                axum::response::Response::builder().status(500).body(axum::body::Body::empty()).unwrap()
            } else {
                text(SAMPLE)
            }
        })
        .await;
        let found = search(&jackett(&server), &bunny(), &Net::new(), TIMEOUT).await.unwrap();
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 3);
        assert!(seen[1..].iter().all(|u| u.starts_with("/api/v2.0/indexers/all/results/torznab/api?")));
        assert_eq!(found.results.len(), 2);
    }

    #[tokio::test]
    async fn other_torznab_address_is_asked_directly() {
        let server = testserver::start(|_| text(SAMPLE)).await;
        let t = Torznab { url: format!("{}/1/api", server.base), ..jackett(&server) };
        let found = search(&t, &bunny(), &Net::new(), TIMEOUT).await.unwrap();
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|u| u.starts_with("/1/api?") && param(u, "t").as_deref() == Some("search")));
        assert_eq!(found.results.len(), 2);
    }
}
