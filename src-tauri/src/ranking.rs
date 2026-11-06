//! Ranking that lifts Turkish content. Other languages are never dropped; only
//! results that look Turkish are flagged so the UI can move them up.

use crate::model::SearchResult;
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

static TR_CHARS: Lazy<Regex> = Lazy::new(|| Regex::new(r"[çğıöşüÇĞİÖŞÜ]").unwrap());
// Word edges also count Turkish letters, so a letter/digit check stands in for \b.
static TR_WORDS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[^\p{L}\p{N}])(t[uü]rk[cç]e|turkish|dublaj|altyaz[ıi]l?[ıi]?|yerli)(?:[^\p{L}\p{N}]|$)").unwrap()
});
// A bare "TR" (e.g. Intro.TR.2020) is not Turkish; only joined tags are.
static TR_TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|[^\p{L}\p{N}])tr[\s._-]?(dub|dublaj|altyaz[ıi]|sub)").unwrap());
static TR_LANG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(tur|tr|turkish|t[uü]rk[cç]e)$").unwrap());
static RESOLUTION_BONUS: Lazy<Vec<(Regex, f64)>> = Lazy::new(|| {
    vec![
        (Regex::new(r"(?i)2160p|(?-u:\b)4k(?-u:\b)|uhd").unwrap(), 0.4),
        (Regex::new(r"(?i)1080p").unwrap(), 0.3),
        (Regex::new(r"(?i)720p").unwrap(), 0.15),
    ]
});
static LOW_QUALITY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?-u:\b)(cam|hdcam|camrip|telesync|hdts|tsrip|ts-rip)(?-u:\b)").unwrap());
// A plain "ts" is usually just the .ts video extension; it counts as a telesync
// tag only after a year or before release details such as codec or resolution.
static TS_AFTER_YEAR: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?-u:\b)(?:19|20)\d{2}[)\]]?[\s_-]+ts(?-u:\b)").unwrap());
static TS_EXT_AFTER_YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?-u:\b)(?:19|20)\d{2}[)\]]?\.ts(?-u:\b)").unwrap());
static TS_BEFORE_DETAILS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?-u:\b)ts[\s._-]+(?:x26[45]|h\.?26[45]|xvid|divx|aac|ac3|mp3|\d{3,4}p|line|audio)(?-u:\b)").unwrap()
});

fn ts_tag(title: &str) -> bool {
    if TS_AFTER_YEAR.is_match(title) || TS_BEFORE_DETAILS.is_match(title) {
        return true;
    }
    // "2023.ts" at the very end is a file name, not a release tag.
    TS_EXT_AFTER_YEAR.find_iter(title).any(|m| !title[m.end()..].trim().is_empty())
}

pub fn is_turkish(r: &SearchResult) -> bool {
    if r.language.iter().any(|l| TR_LANG.is_match(l.trim())) {
        return true;
    }
    let text = format!("{} {}", r.title, r.description);
    TR_CHARS.is_match(&r.title) || TR_WORDS.is_match(&text) || TR_TAGS.is_match(&text)
}

pub fn score(r: &SearchResult) -> f64 {
    let mut s = r.relevance;
    if let Some(n) = r.seeders {
        s += ((n + 1) as f64).log10() * 2.0;
    }
    if let Some(n) = r.downloads {
        s += ((n + 1) as f64).log10();
    }
    if let Some((_, bonus)) = RESOLUTION_BONUS.iter().find(|(re, _)| re.is_match(&r.title)) {
        s += bonus;
    }
    if LOW_QUALITY.is_match(&r.title) || ts_tag(&r.title) {
        s -= 0.8;
    }
    s
}

/// Drops duplicates and sorts by score. Turkish-first is a switch in the UI;
/// the server only sets the `turkish` flag.
pub fn rank_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    let mut unique: Vec<(f64, SearchResult)> = vec![];
    for mut r in results {
        let key = r
            .info_hash
            .clone()
            .or_else(|| r.torrent_url.clone())
            .or_else(|| r.magnet.clone())
            .unwrap_or_else(|| format!("{}:{}", r.source, r.title));
        if !seen.insert(key) {
            continue;
        }
        r.turkish = is_turkish(&r);
        unique.push((score(&r), r));
    }
    // Stable sort keeps the original order for equal scores, like JavaScript.
    unique.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    unique.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
pub(crate) fn sample(id: &str, title: &str) -> SearchResult {
    SearchResult { id: id.into(), title: title.into(), torrent_url: Some(format!("u-{id}")), ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(id: &str, title: &str, lang: &[&str], relevance: f64) -> SearchResult {
        SearchResult { language: lang.iter().map(|s| s.to_string()).collect(), relevance, ..sample(id, title) }
    }

    #[test]
    fn turkish_results_are_flagged_and_order_follows_score() {
        let ranked = rank_results(vec![
            with("a", "Nosferatu", &["English"], 3.0),
            with("b", "Nosferatu (Türkçe Altyazılı)", &[], 1.0),
            with("c", "Nosferatu", &["tur"], 0.5),
            with("d", "Nosferatu Eine Symphonie", &["German"], 2.0),
        ]);
        assert_eq!(ranked.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["a", "d", "b", "c"]);
        assert_eq!(ranked.iter().map(|r| r.turkish).collect::<Vec<_>>(), [false, false, true, true]);
    }

    #[test]
    fn same_torrent_twice_is_kept_once() {
        let mut a = sample("x", "A");
        let mut b = sample("y", "A");
        a.torrent_url = Some("same".into());
        b.torrent_url = Some("same".into());
        assert_eq!(rank_results(vec![a, b]).len(), 1);
    }

    #[test]
    fn dub_and_subtitle_words_count_as_turkish_but_bare_tr_does_not() {
        let t = |title: &str| is_turkish(&sample("x", title));
        assert!(t("Film 1080p TR Dublaj"));
        assert!(is_turkish(&with("x", "Movie", &["Turkish"], 0.0)));
        assert!(!t("The General 1926"));
        assert!(!t("Intro.TR.2020"));
        assert!(!t("Movie.2020.1080p.TR"));
        assert!(t("Film Türkçe Dublaj"));
        assert!(t("Film.2020.TRDublaj.720p"));
        assert!(t("Film 2020 tr altyazı"));
        assert!(t("Film Altyazili 1080p"));
    }

    #[test]
    fn resolution_raises_score_and_camera_rips_lower_it() {
        let items = ["Film 2024 HDCAM", "Film 2024", "Film 2024 720p", "Film 2024 1080p", "Film 2024 2160p"];
        let ids = ["cam", "sd", "hd", "fhd", "uhd"];
        let ranked = rank_results(items.iter().zip(ids).map(|(t, id)| with(id, t, &[], 1.0)).collect());
        assert_eq!(ranked.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["uhd", "fhd", "hd", "sd", "cam"]);
    }

    #[test]
    fn plain_ts_extension_is_not_a_camera_rip_but_ts_tag_is() {
        let penalized = |title: &str| rank_results(vec![sample("x", title), sample("y", "Film 2023")])[0].id == "y";
        assert!(!penalized("Bolum1.ts"));
        assert!(!penalized("Kayit.2023.ts"));
        assert!(!penalized("Teen.Titans.Go.S01"));
        assert!(penalized("Film.2023.TS.x264"));
        assert!(penalized("Film (2023) TS"));
        assert!(penalized("Film.TS.XviD"));
        assert!(penalized("Film 2023 HDTS 720p"));
        assert!(penalized("Film 2023 Telesync"));
    }
}
