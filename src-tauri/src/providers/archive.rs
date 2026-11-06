//! Internet Archive: public-domain and freely licensed content. Every item has
//! a .torrent with a web seed, so it downloads even when there are no peers.

use crate::categories::{category, genre_terms};
use crate::model::{Rating, SearchResult};
use crate::net::{Net, SourceError};
use crate::search::SearchParams;
use once_cell::sync::Lazy;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use serde_json::Value;
use std::time::Duration;

const FIELDS: &[&str] = &[
    "identifier", "title", "item_size", "downloads", "language", "year", "mediatype", "description", "subject", "avg_rating",
    "num_reviews", "licenseurl",
];
const TR_LANGUAGE: &str = r#"language:(tur OR turkish OR "Türkçe" OR "Turkce")"#;
pub const TIMEOUT: Duration = Duration::from_secs(12);

fn quote(term: &str) -> String {
    format!("\"{}\"", term.replace(['"', '\\'], " ").trim())
}

static SPECIAL: Lazy<Regex> = Lazy::new(|| Regex::new(r#"[()":\\/+\-!{}\[\]^~*?&|]"#).unwrap());
static OPERATORS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?-u:\b)(AND|OR|NOT|TO)(?-u:\b)").unwrap());
static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

/// Strips Lucene syntax from free text so brackets, quotes, wildcards and
/// AND/OR/NOT never break the search (HTTP 500).
pub fn clean_text(query: &str) -> String {
    let s = SPECIAL.replace_all(query, " ");
    let s = OPERATORS.replace_all(&s, |c: &regex::Captures| c[1].to_lowercase());
    SPACES.replace_all(&s, " ").trim().to_string()
}

fn first_text(v: Option<&Value>) -> String {
    let v = match v {
        Some(Value::Array(a)) => a.first(),
        other => other,
    };
    let s = match v {
        None | Some(Value::Null) => return String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    TAGS.replace_all(&s, "").trim().to_string()
}

/// Short badge text from a licence URL; empty for unknown licences (never guessed).
pub fn license_label(url: Option<&Value>) -> String {
    let u = first_text(url).to_lowercase();
    if u.contains("publicdomain") {
        "Kamu malı".into()
    } else if u.contains("creativecommons.org") {
        "CC".into()
    } else {
        String::new()
    }
}

pub fn build_query(p: &SearchParams, turkish_only: bool) -> String {
    let mut parts = vec![];
    let text = clean_text(&p.query);
    if !text.is_empty() {
        parts.push(format!("({text})"));
    }
    let terms = genre_terms(&p.genre);
    if !terms.is_empty() {
        parts.push(format!("subject:({})", terms.iter().map(|t| quote(t)).collect::<Vec<_>>().join(" OR ")));
    }
    let cat = category(&p.category);
    if !cat.archive_subjects.is_empty() {
        parts.push(format!("subject:({})", cat.archive_subjects.iter().map(|t| quote(t)).collect::<Vec<_>>().join(" OR ")));
    }
    parts.push(format!("mediatype:({})", cat.archive.join(" OR ")));
    if turkish_only {
        parts.push(TR_LANGUAGE.into());
    }
    parts.join(" AND ")
}

fn enc(s: &str) -> String {
    // Same characters as JavaScript's encodeURIComponent.
    const SET: &percent_encoding::AsciiSet =
        &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'!').remove(b'~').remove(b'*').remove(b'\'').remove(b'(').remove(b')');
    utf8_percent_encode(s, SET).to_string()
}

pub fn archive_poster(id: &str) -> String {
    format!("https://archive.org/services/img/{}", enc(id))
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse().ok(),
        Some(Value::Array(a)) => num(a.first()),
        _ => None,
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a.iter().map(|x| x.as_str().map(String::from).unwrap_or_else(|| x.to_string())).collect(),
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Null) | None => vec![],
        Some(other) => vec![other.to_string()],
    }
}

pub fn map_doc(doc: &Value, index: usize, total: usize) -> SearchResult {
    let id = doc.get("identifier").and_then(Value::as_str).unwrap_or_default().to_string();
    let title = first_text(doc.get("title"));
    let reviews = num(doc.get("num_reviews")).unwrap_or(0.0);
    let avg = num(doc.get("avg_rating")).unwrap_or(0.0);
    let license = license_label(doc.get("licenseurl"));
    SearchResult {
        id: format!("archive:{id}"),
        source: "Internet Archive".into(),
        title: if title.is_empty() { id.clone() } else { title },
        description: first_text(doc.get("description")).chars().take(220).collect(),
        year: num(doc.get("year")).filter(|y| *y != 0.0).map(|y| y as i32),
        language: strings(doc.get("language")),
        category: doc.get("mediatype").and_then(Value::as_str).unwrap_or_default().to_string(),
        size: num(doc.get("item_size")).filter(|n| *n > 0.0).map(|n| n as u64),
        seeders: None,
        downloads: Some(num(doc.get("downloads")).unwrap_or(0.0) as u64),
        subjects: strings(doc.get("subject")).into_iter().take(20).collect(),
        // Archive ratings are out of 5; shown out of 10.
        rating: (reviews >= 2.0 && avg != 0.0).then(|| Rating { value: avg * 2.0, source: "Archive".into(), votes: Some(reviews as u64) }),
        license: Some(license),
        web_seed: true,
        torrent_url: Some(format!("https://archive.org/download/{0}/{0}_archive.torrent", enc(&id))),
        page_url: Some(format!("https://archive.org/details/{}", enc(&id))),
        // Archive serves a cover for every item, so a poster shows even without an OMDb key.
        poster: Some(archive_poster(&id)),
        relevance: ((total - index) as f64 / total as f64) * 3.0,
        ..Default::default()
    }
}

async fn fetch_docs(net: &Net, q: &str, rows: u32, sort: &str, timeout: Duration) -> Result<Vec<Value>, SourceError> {
    let mut url = url::Url::parse(&net.archive_api).map_err(|e| SourceError::msg(e.to_string()))?;
    {
        let mut qp = url.query_pairs_mut();
        qp.append_pair("q", q);
        for f in FIELDS {
            qp.append_pair("fl[]", f);
        }
        qp.append_pair("rows", &rows.to_string());
        qp.append_pair("output", "json");
        if !sort.is_empty() {
            qp.append_pair("sort[]", sort);
        }
    }
    let res = net.client.get(url).header("Accept", "application/json").timeout(timeout).send().await?;
    if !res.status().is_success() {
        return Err(SourceError::msg(format!("Internet Archive {}", res.status().as_u16())));
    }
    let data: Value = res.json().await?;
    Ok(data.pointer("/response/docs").and_then(Value::as_array).cloned().unwrap_or_default())
}

pub async fn search(p: &SearchParams, net: &Net, timeout: Duration) -> Result<Vec<SearchResult>, SourceError> {
    // Only special characters: return nothing instead of everything.
    if !p.query.trim().is_empty() && clean_text(&p.query).is_empty() && p.genre.is_empty() {
        return Ok(vec![]);
    }
    // Genre only: most popular first; otherwise relevance order.
    let sort = if p.query.trim().is_empty() { "downloads desc" } else { "" };
    // A separate query makes sure Turkish results are there; the general one
    // brings content in other languages.
    let (q_tr, q_all) = (build_query(p, true), build_query(p, false));
    let (tr, all) = tokio::join!(fetch_docs(net, &q_tr, 20, sort, timeout), fetch_docs(net, &q_all, 40, sort, timeout));
    let tr = tr.unwrap_or_default();
    let all = all?;
    let mut out: Vec<SearchResult> = tr
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let mut r = map_doc(d, i, tr.len());
            if r.language.is_empty() {
                r.language = vec!["tur".into()];
            }
            r
        })
        .collect();
    out.extend(all.iter().enumerate().map(|(i, d)| map_doc(d, i, all.len())));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::testserver::{self, json, param};
    use crate::ranking::rank_results;
    use serde_json::json;

    fn params(query: &str, category: &str, genre: &str) -> SearchParams {
        SearchParams { query: query.into(), category: category.into(), genre: genre.into() }
    }

    #[test]
    fn query_has_genre_and_category() {
        let q = build_query(&params("chaplin", "movies", "komedi"), true);
        assert!(q.contains("(chaplin)"));
        assert!(q.contains(r#"subject:("komedi" OR "comedy")"#));
        assert!(q.contains("mediatype:(movies)"));
        assert!(q.contains("language:(tur"));
        assert!(build_query(&params("", "adult", ""), false).contains(r#"subject:("erotic""#));
        assert!(build_query(&params("kurtlar", "tv", ""), false).contains(r#"subject:("television" OR "tv series" OR "dizi" OR "tv show")"#));
        assert!(!build_query(&params("kurtlar", "movies", ""), false).contains("television"));
    }

    #[test]
    fn licence_badge_only_when_there_is_a_licence_url() {
        let lic = |v: Value| map_doc(&v, 0, 1).license.unwrap();
        assert_eq!(lic(json!({"identifier": "a", "licenseurl": "http://creativecommons.org/publicdomain/mark/1.0/"})), "Kamu malı");
        assert_eq!(lic(json!({"identifier": "b", "licenseurl": "https://creativecommons.org/licenses/by-sa/4.0/"})), "CC");
        assert_eq!(lic(json!({"identifier": "c"})), "");
        assert_eq!(lic(json!({"identifier": "d", "licenseurl": "http://example.com/terms"})), "");
    }

    #[test]
    fn archive_results_have_posters() {
        let r = map_doc(&json!({"identifier": "nosferatu_1922"}), 0, 1);
        assert_eq!(r.poster.as_deref(), Some("https://archive.org/services/img/nosferatu_1922"));
    }

    #[tokio::test]
    async fn lucene_operators_and_special_characters_do_not_break_the_query() {
        assert_eq!(clean_text(r#""Tom AND Jerry" (1940) OR NOT x*"#), "Tom and Jerry 1940 or not x");
        let server = testserver::start(|_| json(json!({"response": {"docs": []}}))).await;
        let out = search(&params(r#""()" :"#, "all", ""), &Net::local(&server.base), TIMEOUT).await.unwrap();
        assert!(out.is_empty());
        assert!(server.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn turkish_and_general_queries_are_combined() {
        let server = testserver::start(|req| {
            let q = param(&req.uri().to_string(), "q").unwrap();
            if q.contains("language:") {
                json(json!({"response": {"docs": [{"identifier": "tr-film", "title": "Türk Filmi", "item_size": 1000, "downloads": 5}]}}))
            } else {
                json(json!({"response": {"docs": [{"identifier": "en-film", "title": "English Film", "item_size": 2000, "downloads": 50, "language": "English", "mediatype": "movies"}]}}))
            }
        })
        .await;
        let results = rank_results(search(&params("film", "all", ""), &Net::local(&server.base), TIMEOUT).await.unwrap());
        assert_eq!(server.seen.lock().unwrap().len(), 2);
        let tr = results.iter().find(|r| r.title == "Türk Filmi").unwrap();
        assert!(tr.turkish);
        assert_eq!(tr.torrent_url.as_deref(), Some("https://archive.org/download/tr-film/tr-film_archive.torrent"));
        assert!(!results.iter().find(|r| r.title == "English Film").unwrap().turkish);
    }
}
