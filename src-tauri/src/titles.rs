//! Readable name and year from torrent names ("Film.Adi.2019.1080p.BluRay.x264").

use once_cell::sync::Lazy;
use regex::Regex;

// `(?-u:\b)` is an ASCII word boundary, the same as JavaScript's `\b`.
static NOISE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?-u:\b)(2160p|1080p|720p|480p|4k|uhd|hdr|bluray|blu-ray|brrip|bdrip|web-?dl|webrip|web|hdtv|dvdrip|dvd|x264|x265|h\.?264|h\.?265|hevc|aac|ac3|dts|remux|proper|repack|extended|unrated|türkçe|turkce|dublaj|altyazı|altyazılı|tr|multi|dual)(?-u:\b)").unwrap()
});
static BRACKETS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[[^\]]*\]").unwrap());
static DOTS: Lazy<Regex> = Lazy::new(|| Regex::new(r"[._]+").unwrap());
static SERIES: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?-u:\b)S\d{1,2}(E\d{1,3})?(?-u:\b)|(?-u:\b)sezon(?-u:\b)|(?-u:\b)season(?-u:\b)").unwrap());
static YEAR: Lazy<Regex> = Lazy::new(|| Regex::new(r"[(\[\s.](19[0-9]{2}|20[0-9]{2})(?:[)\]\s.]|$)").unwrap());
static CUTS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        Regex::new(r"[(\[\s.](19|20)\d{2}(?:[)\]\s.]|$)").unwrap(),
        Regex::new(r"(?i)(?-u:\b)S\d{1,2}(E\d{1,3})?(?-u:\b)").unwrap(),
        Regex::new(r"(?i)(?-u:\b)(season|sezon)\s*\d+").unwrap(),
        Regex::new(r"(?i)(?-u:\b)complete(?-u:\b)").unwrap(),
        NOISE.clone(),
        Regex::new(r"\s[-–—:]\s").unwrap(),
    ]
});
static TAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"[(\[]\s*$").unwrap());
static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub name: String,
    pub year: Option<i32>,
    pub series: bool,
}

pub fn parse_title(raw: &str) -> Parsed {
    let mut title = BRACKETS.replace_all(raw, " ").into_owned();
    if !title.trim().chars().any(char::is_whitespace) {
        title = DOTS.replace_all(&title, " ").into_owned();
    }
    let series = SERIES.is_match(&title);
    let year = YEAR.captures(&title).and_then(|c| c[1].parse().ok());
    let mut cut = title.len();
    for re in CUTS.iter() {
        if let Some(m) = re.find(&title) {
            if m.start() > 0 {
                cut = cut.min(m.start());
            }
        }
    }
    let head = TAIL.replace(&title[..cut], "");
    let name = SPACES.replace_all(&head, " ").trim().to_string();
    Parsed { name: if name.is_empty() { title.trim().to_string() } else { name }, year, series }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_year_from_torrent_names() {
        let p = parse_title("The.Matrix.1999.1080p.BluRay.x264");
        assert_eq!((p.name.as_str(), p.year, p.series), ("The Matrix", Some(1999), false));
        let p = parse_title("Breaking Bad S01E02 720p");
        assert_eq!((p.name.as_str(), p.series), ("Breaking Bad", true));
        let p = parse_title("[TR] Babam ve Oğlum (2005) Türkçe Dublaj");
        assert_eq!((p.name.as_str(), p.year), ("Babam ve Oğlum", Some(2005)));
        assert_eq!(parse_title("Nosferatu").name, "Nosferatu");
    }
}
