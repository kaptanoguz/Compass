//! Category and genre shortcuts shown in the UI. A Turkish genre name is also
//! searched with its English tags so content in other languages is found too.

use once_cell::sync::Lazy;
use regex::Regex;

pub struct Category {
    pub key: &'static str,
    pub label: &'static str,
    pub archive: &'static [&'static str],
    pub archive_subjects: &'static [&'static str],
    pub torznab: &'static [u32],
}

pub static CATEGORIES: &[Category] = &[
    Category { key: "all", label: "Hepsi", archive: &["movies", "audio", "texts", "software"], archive_subjects: &[], torznab: &[] },
    Category { key: "movies", label: "Film", archive: &["movies"], archive_subjects: &[], torznab: &[2000] },
    Category { key: "tv", label: "Dizi", archive: &["movies"], archive_subjects: &["television", "tv series", "dizi", "tv show"], torznab: &[5000] },
    Category { key: "music", label: "Müzik", archive: &["audio"], archive_subjects: &[], torznab: &[3000] },
    Category { key: "books", label: "Kitap", archive: &["texts"], archive_subjects: &[], torznab: &[7000] },
    Category { key: "software", label: "Yazılım", archive: &["software"], archive_subjects: &[], torznab: &[4000] },
    Category {
        key: "adult",
        label: "+18",
        archive: &["movies", "texts"],
        archive_subjects: &["erotic", "erotica", "erotik", "adult", "nudity", "pin-up", "burlesque", "sexploitation"],
        torznab: &[6000],
    },
];

pub fn category(key: &str) -> &'static Category {
    CATEGORIES.iter().find(|c| c.key == key).unwrap_or(&CATEGORIES[0])
}

pub struct Genre {
    pub key: &'static str,
    pub label: &'static str,
    pub terms: &'static [&'static str],
}

pub static GENRES: &[Genre] = &[
    Genre { key: "aksiyon", label: "Aksiyon", terms: &["aksiyon", "action"] },
    Genre { key: "komedi", label: "Komedi", terms: &["komedi", "comedy"] },
    Genre { key: "dram", label: "Dram", terms: &["dram", "drama"] },
    Genre { key: "korku", label: "Korku", terms: &["korku", "horror"] },
    Genre { key: "gerilim", label: "Gerilim", terms: &["gerilim", "thriller", "suspense"] },
    Genre { key: "bilim-kurgu", label: "Bilim Kurgu", terms: &["bilim kurgu", "science fiction", "sci-fi"] },
    Genre { key: "romantik", label: "Romantik", terms: &["romantik", "romance"] },
    Genre { key: "animasyon", label: "Animasyon", terms: &["animasyon", "animation", "cartoon"] },
    Genre { key: "belgesel", label: "Belgesel", terms: &["belgesel", "documentary"] },
    Genre { key: "suc", label: "Suç", terms: &["suç", "crime", "film noir", "noir"] },
    Genre { key: "macera", label: "Macera", terms: &["macera", "adventure"] },
    Genre { key: "fantastik", label: "Fantastik", terms: &["fantastik", "fantasy"] },
    Genre { key: "western", label: "Western", terms: &["western", "kovboy"] },
    Genre { key: "muzikal", label: "Müzikal", terms: &["müzikal", "musical"] },
    Genre { key: "klasik", label: "Klasik", terms: &["klasik", "classic"] },
    Genre { key: "yesilcam", label: "Yeşilçam", terms: &["yeşilçam", "yesilcam", "türk filmi", "turkish film"] },
];

pub fn genre_terms(genre: &str) -> &'static [&'static str] {
    GENRES.iter().find(|g| g.key == genre).map(|g| g.terms).unwrap_or(&[])
}

static GENRE_RES: Lazy<Vec<(&'static str, Vec<Regex>)>> = Lazy::new(|| {
    GENRES
        .iter()
        .filter(|g| g.key != "klasik")
        .map(|g| {
            let res = g.terms.iter().map(|t| Regex::new(&format!(r"(^|[^\p{{L}}]){}([^\p{{L}}]|$)", regex::escape(t))).unwrap()).collect();
            (g.key, res)
        })
        .collect()
});

/// Works out a result's genres from subject tags, OMDb genres and the title.
pub fn detect_genres<S: AsRef<str>>(texts: &[S]) -> Vec<String> {
    let hay = texts.iter().map(|s| s.as_ref()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" | ").to_lowercase();
    if hay.is_empty() {
        return vec![];
    }
    GENRE_RES.iter().filter(|(_, res)| res.iter().any(|r| r.is_match(&hay))).map(|(k, _)| k.to_string()).collect()
}

static ADULT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(^|[^\p{L}])(xxx|porn\w*|erotic\w*|erotik\w*|nsfw|adult|hentai|sexploitation|\+18|18\+)([^\p{L}]|$)").unwrap()
});

pub fn is_adult(title: &str, subjects: &[String], torznab_category: Option<u32>) -> bool {
    if let Some(c) = torznab_category {
        if (6000..7000).contains(&c) {
            return true;
        }
    }
    let mut parts = vec![title.to_string()];
    parts.extend(subjects.iter().cloned());
    ADULT.is_match(&parts.join(" | "))
}

pub fn category_from_torznab(cat: Option<u32>) -> &'static str {
    match cat.unwrap_or(0) {
        2000..=2999 => "movies",
        5000..=5999 => "tv",
        3000..=3999 => "audio",
        7000..=7999 => "texts",
        4000..=4999 => "software",
        _ => "movies",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genres_come_from_subject_tags() {
        assert_eq!(detect_genres(&["Horror", "Comedy films"]), vec!["komedi", "korku"]);
        assert_eq!(detect_genres(&["Bilim Kurgu"]), vec!["bilim-kurgu"]);
        assert!(detect_genres(&["dramatic"]).is_empty(), "whole words only");
        assert!(detect_genres::<&str>(&[]).is_empty());
    }

    #[test]
    fn adult_content_is_recognised() {
        assert!(is_adult("Some XXX title", &[], None));
        assert!(is_adult("Film", &["Erotica".into()], None));
        assert!(is_adult("Film", &[], Some(6010)));
        assert!(!is_adult("Adultery drama", &[], None));
        assert!(!is_adult("Film", &[], Some(2000)));
    }
}
