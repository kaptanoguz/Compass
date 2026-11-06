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

pub const TIMEOUT: Duration = Duration::from_secs(12);

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

async fn run(t: &Torznab, net: &Net, q: &str, cat: &str, timeout: Duration) -> Result<Vec<SearchResult>, SourceError> {
    let name = if t.name.is_empty() { "Torznab" } else { &t.name };
    let mut url = url::Url::parse(&endpoint(&t.url)).map_err(|e| SourceError::msg(format!("{name}: {e}")))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("t", "search");
        qp.append_pair("q", q);
        if !t.api_key.is_empty() {
            qp.append_pair("apikey", &t.api_key);
        }
        let cats = category(cat).torznab;
        if !cats.is_empty() {
            qp.append_pair("cat", &cats.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(","));
        }
    }
    let res = net.client.get(url).timeout(timeout).send().await?;
    if !res.status().is_success() {
        return Err(SourceError::msg(format!("{name} {}", res.status().as_u16())));
    }
    Ok(parse_torznab(&res.text().await?, name))
}

pub async fn search(t: &Torznab, p: &SearchParams, net: &Net, timeout: Duration) -> Result<Vec<SearchResult>, SourceError> {
    let terms = [p.query.trim(), genre_terms(&p.genre).get(1).copied().unwrap_or("")]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if terms.is_empty() {
        return Ok(vec![]);
    }
    // Turkish versions are also tried; if that extra query fails the main
    // results still come back. The same infohash in both lists is kept once.
    let tr_terms = format!("{terms} türkçe");
    let (all, tr) = tokio::join!(run(t, net, &terms, &p.category, timeout), run(t, net, &tr_terms, &p.category, timeout));
    let mut all = all?;
    all.extend(tr.unwrap_or_default());
    let mut seen = HashSet::new();
    Ok(all
        .into_iter()
        .filter(|r| seen.insert(r.info_hash.clone().or_else(|| r.magnet.clone()).or_else(|| r.torrent_url.clone()).unwrap_or_default()))
        .collect())
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

    #[tokio::test]
    async fn provider_sends_api_key_and_category() {
        let server = testserver::start(|_| text(SAMPLE)).await;
        let t = Torznab {
            id: "t".into(),
            name: "Jackett".into(),
            url: format!("{}/api/v2.0/indexers/all/results/torznab", server.base),
            api_key: "k".into(),
            enabled: true,
        };
        let p = SearchParams { query: "bunny".into(), category: "movies".into(), genre: String::new() };
        let results = search(&t, &p, &Net::new(), TIMEOUT).await.unwrap();
        let seen = server.seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|u| u.starts_with("/api/v2.0/indexers/all/results/torznab/api?")));
        assert!(seen.iter().all(|u| param(u, "apikey").as_deref() == Some("k") && param(u, "cat").as_deref() == Some("2000")));
        let mut qs: Vec<_> = seen.iter().map(|u| param(u, "q").unwrap()).collect();
        qs.sort();
        assert_eq!(qs, ["bunny", "bunny türkçe"]);
        assert_eq!(results.len(), 2, "same results from both queries are kept once");
        assert!(rank_results(results).iter().find(|r| r.title == "Sintel TR Dublaj").unwrap().turkish);
    }
}
