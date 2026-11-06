//! Runs every enabled source at once and merges the results. Sorting and
//! filtering happen in the UI; the server only sets a sensible upper limit.

use crate::categories::{detect_genres, is_adult};
use crate::model::SearchResult;
use crate::net::{Net, SourceError};
use crate::providers::{archive, torznab};
use crate::ranking::rank_results;
use crate::ratings::{add_imdb_ratings, RatingsCache};
use crate::settings::{Settings, Torznab};
use crate::titles::parse_title;
use serde::Serialize;
use std::sync::Arc;
use std::time::Duration;

pub const MAX_RESULTS: usize = 120;

#[derive(Clone, Debug, Default)]
pub struct SearchParams {
    pub query: String,
    pub category: String,
    pub genre: String,
}

#[derive(Clone, Debug)]
pub enum Provider {
    Archive,
    Torznab(Torznab),
    /// Fixed results, for tests.
    #[cfg(test)]
    Fixed(String, Result<Vec<SearchResult>, String>),
}

impl Provider {
    pub fn id(&self) -> String {
        match self {
            Provider::Archive => "archive".into(),
            Provider::Torznab(t) => t.id.clone(),
            #[cfg(test)]
            Provider::Fixed(n, _) => n.clone(),
        }
    }
    pub fn name(&self) -> String {
        match self {
            Provider::Archive => "Internet Archive".into(),
            Provider::Torznab(t) => if t.name.is_empty() { "Torznab".into() } else { t.name.clone() },
            #[cfg(test)]
            Provider::Fixed(n, _) => n.clone(),
        }
    }
    async fn search(&self, p: &SearchParams, net: &Net, timeout: Option<Duration>) -> Result<Vec<SearchResult>, SourceError> {
        match self {
            Provider::Archive => archive::search(p, net, timeout.unwrap_or(archive::TIMEOUT)).await,
            Provider::Torznab(t) => torznab::search(t, p, net, timeout.unwrap_or(torznab::TIMEOUT)).await,
            #[cfg(test)]
            Provider::Fixed(_, r) => r.clone().map_err(SourceError::msg),
        }
    }
}

pub fn active_providers(s: &Settings) -> Vec<Provider> {
    let mut list = vec![];
    if s.archive_enabled {
        list.push(Provider::Archive);
    }
    for t in &s.torznab {
        if t.enabled {
            list.push(Provider::Torznab(t.clone()));
        }
    }
    list
}

#[derive(Serialize)]
pub struct SourceFailure {
    pub provider: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct SearchOutput {
    pub results: Vec<SearchResult>,
    pub errors: Vec<SourceFailure>,
}

pub struct SearchDeps<'a> {
    pub net: &'a Net,
    pub omdb_api_key: &'a str,
    pub cache: &'a Arc<RatingsCache>,
    pub timeout: Option<Duration>,
    pub ratings_budget: Duration,
}

pub async fn search_all(p: &SearchParams, providers: &[Provider], deps: SearchDeps<'_>) -> SearchOutput {
    let settled = futures::future::join_all(providers.iter().map(|pr| pr.search(p, deps.net, deps.timeout))).await;
    let mut results = vec![];
    let mut errors = vec![];
    for (i, s) in settled.into_iter().enumerate() {
        match s {
            Ok(v) => results.extend(v),
            Err(e) => errors.push(SourceFailure {
                provider: providers[i].name(),
                message: if e.timeout { "Yanıt vermedi (zaman aşımı).".into() } else { e.message },
            }),
        }
    }

    // Adult content shows up only in its own section.
    let adult_mode = p.category == "adult";
    let filtered: Vec<SearchResult> = rank_results(results)
        .into_iter()
        .map(|mut r| {
            r.adult = adult_mode || is_adult(&r.title, &r.subjects, r.torznab_category);
            r
        })
        .filter(|r| adult_mode || !r.adult)
        .collect();
    // If the list is cut, Turkish results stay in it; the order is still by score.
    let mut keep = vec![false; filtered.len()];
    let order = (0..filtered.len()).filter(|&i| filtered[i].turkish).chain((0..filtered.len()).filter(|&i| !filtered[i].turkish));
    for i in order.take(MAX_RESULTS) {
        keep[i] = true;
    }
    let mut ranked: Vec<SearchResult> = filtered.into_iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, r)| r).collect();

    for r in ranked.iter_mut() {
        if r.year.is_none() {
            r.year = parse_title(&r.title).year;
        }
    }
    add_imdb_ratings(&mut ranked, deps.omdb_api_key, deps.net, deps.cache, deps.ratings_budget).await;
    for r in ranked.iter_mut() {
        r.genres = detect_genres(&r.subjects);
    }
    SearchOutput { results: ranked, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ranking::sample;

    async fn run(p: SearchParams, providers: Vec<Provider>) -> SearchOutput {
        let net = Net::local("http://127.0.0.1:9");
        let cache = Arc::new(RatingsCache::open(None));
        search_all(&p, &providers, SearchDeps { net: &net, omdb_api_key: "", cache: &cache, timeout: None, ratings_budget: Duration::from_millis(10) }).await
    }

    fn q(query: &str, category: &str) -> SearchParams {
        SearchParams { query: query.into(), category: category.into(), genre: String::new() }
    }

    #[tokio::test]
    async fn adult_results_are_hidden_in_normal_search_and_shown_in_their_section() {
        let items = vec![
            SearchResult { year: Some(1968), subjects: vec!["Horror".into()], ..sample("1", "Night of the Living Dead") },
            SearchResult { subjects: vec!["erotica".into()], ..sample("2", "Erotica Reel") },
        ];
        let normal = run(q("x", "all"), vec![Provider::Fixed("Test".into(), Ok(items.clone()))]).await;
        assert_eq!(normal.results.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["1"]);
        assert_eq!(normal.results[0].genres, ["korku"]);
        let adult = run(q("", "adult"), vec![Provider::Fixed("Test".into(), Ok(items))]).await;
        assert_eq!(adult.results.len(), 2);
        assert!(adult.results.iter().all(|r| r.adult));
    }

    #[tokio::test]
    async fn year_is_read_from_title_when_missing() {
        let out = run(q("m", "all"), vec![Provider::Fixed("T".into(), Ok(vec![sample("1", "Metropolis.1927.720p")]))]).await;
        assert_eq!(out.results[0].year, Some(1927));
    }

    #[tokio::test]
    async fn results_are_capped_at_120_and_80_are_not_cut() {
        let many = |n: usize, prefix: &str| {
            Provider::Fixed(
                prefix.into(),
                Ok((0..n).map(|i| SearchResult { relevance: i as f64 / n as f64, ..sample(&format!("{prefix}{i}"), &format!("{prefix} Film {i}")) }).collect()),
            )
        };
        assert_eq!(run(q("film", "all"), vec![many(80, "a")]).await.results.len(), 80);
        assert_eq!(run(q("film", "all"), vec![many(100, "a"), many(100, "b")]).await.results.len(), MAX_RESULTS);
    }

    #[tokio::test]
    async fn failing_source_is_reported_and_others_still_return() {
        let out = run(q("x", "all"), vec![Provider::Fixed("Bozuk".into(), Err("Bozuk 500".into())), Provider::Fixed("İyi".into(), Ok(vec![sample("1", "Film")]))]).await;
        assert_eq!(out.results.len(), 1);
        assert_eq!(out.errors[0].provider, "Bozuk");
    }
}
