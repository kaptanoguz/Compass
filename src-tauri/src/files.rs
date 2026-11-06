//! What kind of file is this: video, subtitle, program? Also the guard against
//! fake "films" that are really programs, and SRT → WebVTT conversion.

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::path::Path;

pub fn ext(name: &str) -> String {
    Path::new(name).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

pub fn video_type(name: &str) -> Option<&'static str> {
    Some(match ext(name).as_str() {
        ".mp4" | ".m4v" => "video/mp4",
        ".mov" => "video/quicktime",
        ".webm" => "video/webm",
        ".mkv" => "video/x-matroska",
        ".ogv" => "video/ogg",
        ".avi" => "video/x-msvideo",
        ".wmv" => "video/x-ms-wmv",
        ".mpg" | ".mpeg" => "video/mpeg",
        ".ts" => "video/mp2t",
        _ => return None,
    })
}

static HEVC: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(^|[^a-z0-9])(hevc|x265|h\.?265)([^a-z0-9]|$)").unwrap());

/// Containers the built-in player opens safely. HEVC/x265 files often fail even inside mp4.
pub fn playable_in_browser(name: &str) -> bool {
    matches!(ext(name).as_str(), ".mp4" | ".m4v" | ".mov" | ".webm" | ".ogv") && !HEVC.is_match(name)
}

// Executables: a common malware trick in torrents shared as films
// (e.g. "Film.2024.1080p.mp4.exe"). Allowed only in the Software category.
const PROGRAM_EXT: &[&str] = &[
    ".exe", ".msi", ".bat", ".cmd", ".com", ".scr", ".pif", ".cpl", ".msc", ".hta", ".reg", ".dll", ".lnk", ".url", ".vbs", ".vbe",
    ".js", ".jse", ".wsf", ".wsh", ".ps1", ".psm1", ".jar", ".apk", ".sh", ".run", ".appimage", ".deb", ".rpm", ".pkg", ".dmg",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Program,
    Video,
    Subtitle,
    Other,
}

pub fn file_kind(name: &str) -> Kind {
    let e = ext(name);
    if PROGRAM_EXT.contains(&e.as_str()) {
        Kind::Program
    } else if video_type(name).is_some() {
        Kind::Video
    } else if e == ".srt" || e == ".vtt" {
        Kind::Subtitle
    } else {
        Kind::Other
    }
}

pub const BLOCKED_MESSAGE: &str = "Bu torrentte film yerine çalıştırılabilir dosya (.exe gibi) var; güvenlik için indirilmedi.";

#[derive(Debug, PartialEq, Eq)]
pub enum Screen {
    Blocked,
    Keep { keep: Vec<usize>, skipped: usize },
}

static SIDE_FILE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\.(srt|vtt|sub|idx|nfo|txt|url|jpg|jpeg|png|gif|md|html?)$").unwrap());

/// What is left once program files are removed? If nothing, or only side files
/// (subtitles, nfo, txt), the torrent is treated as fake.
/// None: clean. Some(Blocked) or Some(Keep { indexes, skipped }).
pub fn screen_programs<S: AsRef<str>>(names: &[S]) -> Option<Screen> {
    let kinds: Vec<Kind> = names.iter().map(|n| file_kind(n.as_ref())).collect();
    let skipped = kinds.iter().filter(|k| **k == Kind::Program).count();
    if skipped == 0 {
        return None;
    }
    let keep: Vec<usize> = (0..names.len()).filter(|&i| kinds[i] != Kind::Program).collect();
    let has_media = keep.iter().any(|&i| kinds[i] == Kind::Video || !SIDE_FILE.is_match(names[i].as_ref()));
    Some(if has_media { Screen::Keep { keep, skipped } } else { Screen::Blocked })
}

static SRT_TIME: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d{2}:\d{2}:\d{2}),(\d{3})").unwrap());

pub const MAX_SUBTITLE_BYTES: usize = 1024 * 1024;

/// Turns SRT into the WebVTT browsers understand. Turkish subtitles often come
/// in Windows-1254; when UTF-8 looks broken that encoding is tried.
pub fn subtitle_to_vtt(buf: &[u8], name: &str) -> String {
    let text = match std::str::from_utf8(buf) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1254.decode(buf).0.into_owned(),
    };
    let text = text.trim_start_matches('\u{FEFF}').replace("\r\n", "\n").replace('\r', "\n");
    if ext(name) == ".vtt" {
        return text;
    }
    format!("WEBVTT\n\n{}", SRT_TIME.replace_all(&text, "$1.$2"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_files_are_removed_and_torrent_without_film_is_fake() {
        assert_eq!(screen_programs(&["Film.mkv", "Film.srt"]), None);
        assert_eq!(screen_programs(&["Film.mkv", "Codec.exe", "Film.srt"]), Some(Screen::Keep { keep: vec![0, 2], skipped: 1 }));
        assert_eq!(screen_programs(&["Film.2024.1080p.mp4.exe", "Film.nfo", "Okubeni.txt"]), Some(Screen::Blocked));
        assert_eq!(screen_programs(&["setup.msi"]), Some(Screen::Blocked));
        assert_eq!(screen_programs(&["Album.flac", "kur.bat"]), Some(Screen::Keep { keep: vec![0], skipped: 1 }));
    }

    #[test]
    fn srt_becomes_vtt_and_windows_1254_turkish_letters_survive() {
        let mut srt = b"1\r\n00:00:01,000 --> 00:00:02,500\r\n".to_vec();
        srt.extend([0xd0, 0xfc, 0xfe, 0xdd, 0xf6, 0xe7]);
        srt.extend(b"\r\n");
        assert_eq!(subtitle_to_vtt(&srt, "film.tr.srt"), "WEBVTT\n\n1\n00:00:01.000 --> 00:00:02.500\nĞüşİöç\n");
    }

    #[test]
    fn mkv_and_hevc_are_not_playable_in_the_built_in_player() {
        assert!(playable_in_browser("Film.mp4"));
        assert!(!playable_in_browser("Film.mkv"));
        assert!(!playable_in_browser("Film.x265.mp4"));
        assert!(!playable_in_browser("Film.HEVC.mp4"));
    }
}
