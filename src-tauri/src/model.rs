//! A search result as the UI receives it. Missing values are left out of the JSON.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Rating {
    pub value: f64,
    pub source: String,
    pub votes: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub id: String,
    pub source: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,
    #[serde(default)]
    pub language: Vec<String>,
    #[serde(default)]
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torznab_category: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seeders: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peers: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
    #[serde(default)]
    pub subjects: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating: Option<Rating>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub web_seed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub torrent_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magnet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub info_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imdb_url: Option<String>,
    #[serde(default)]
    pub relevance: f64,
    #[serde(default)]
    pub turkish: bool,
    #[serde(default)]
    pub adult: bool,
    #[serde(default)]
    pub genres: Vec<String>,
}
