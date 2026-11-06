//! Reading .torrent files (bencode) and magnet links.

use once_cell::sync::Lazy;
use regex::Regex;
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Bencode {
    Int(i64),
    Bytes(Vec<u8>),
    List(Vec<Bencode>),
    Dict(BTreeMap<Vec<u8>, Bencode>),
}

impl Bencode {
    pub fn get(&self, key: &str) -> Option<&Bencode> {
        match self {
            Bencode::Dict(d) => d.get(key.as_bytes()),
            _ => None,
        }
    }
    pub fn str(&self) -> Option<String> {
        match self {
            Bencode::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
            _ => None,
        }
    }
    pub fn int(&self) -> Option<i64> {
        match self {
            Bencode::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![];
        self.encode_into(&mut out);
        out
    }
    fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Bencode::Int(n) => out.extend(format!("i{n}e").bytes()),
            Bencode::Bytes(b) => {
                out.extend(format!("{}:", b.len()).bytes());
                out.extend(b);
            }
            Bencode::List(l) => {
                out.push(b'l');
                l.iter().for_each(|v| v.encode_into(out));
                out.push(b'e');
            }
            Bencode::Dict(d) => {
                out.push(b'd');
                for (k, v) in d {
                    Bencode::Bytes(k.clone()).encode_into(out);
                    v.encode_into(out);
                }
                out.push(b'e');
            }
        }
    }
}

struct Parser<'a> {
    buf: &'a [u8],
    pos: usize,
    info_span: Option<(usize, usize)>,
    depth: usize,
}

impl Parser<'_> {
    fn err<T>(&self) -> Result<T, String> {
        Err(format!("bencode hatası ({})", self.pos))
    }
    fn peek(&self) -> Option<u8> {
        self.buf.get(self.pos).copied()
    }
    fn number_until(&mut self, end: u8) -> Result<i64, String> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c != end) {
            self.pos += 1;
        }
        if self.peek() != Some(end) {
            return self.err();
        }
        let s = std::str::from_utf8(&self.buf[start..self.pos]).map_err(|e| e.to_string())?;
        self.pos += 1;
        s.parse().map_err(|_| format!("bencode sayı hatası ({start})"))
    }
    fn value(&mut self) -> Result<Bencode, String> {
        self.depth += 1;
        if self.depth > 64 {
            return self.err();
        }
        let v = match self.peek() {
            Some(b'i') => {
                self.pos += 1;
                Bencode::Int(self.number_until(b'e')?)
            }
            Some(b'l') => {
                self.pos += 1;
                let mut list = vec![];
                while self.peek() != Some(b'e') {
                    if self.peek().is_none() {
                        return self.err();
                    }
                    list.push(self.value()?);
                }
                self.pos += 1;
                Bencode::List(list)
            }
            Some(b'd') => {
                self.pos += 1;
                let mut dict = BTreeMap::new();
                while self.peek() != Some(b'e') {
                    let Bencode::Bytes(key) = self.value()? else { return self.err() };
                    let start = self.pos;
                    let v = self.value()?;
                    if self.depth == 1 && key == b"info" {
                        self.info_span = Some((start, self.pos));
                    }
                    dict.insert(key, v);
                }
                self.pos += 1;
                Bencode::Dict(dict)
            }
            Some(c) if c.is_ascii_digit() => {
                let len = self.number_until(b':')? as usize;
                let end = self.pos.checked_add(len).filter(|e| *e <= self.buf.len()).ok_or("bencode uzunluk hatası")?;
                let b = self.buf[self.pos..end].to_vec();
                self.pos = end;
                Bencode::Bytes(b)
            }
            _ => return self.err(),
        };
        self.depth -= 1;
        Ok(v)
    }
}

pub fn decode(buf: &[u8]) -> Result<(Bencode, Option<(usize, usize)>), String> {
    let mut p = Parser { buf, pos: 0, info_span: None, depth: 0 };
    let v = p.value()?;
    Ok((v, p.info_span))
}

#[derive(Clone, Debug)]
pub struct TorrentFile {
    pub name: String,
    pub path: String,
    pub length: u64,
}

#[derive(Clone, Debug)]
pub struct TorrentInfo {
    pub info_hash: String,
    pub name: String,
    pub length: u64,
    pub files: Vec<TorrentFile>,
    pub private: bool,
    pub piece_length: u64,
}

/// Parses a .torrent file. Paths are "name/sub/file" for multi-file torrents
/// and just the name for a single file, like qBittorrent's "Original" layout.
pub fn parse_torrent(buf: &[u8]) -> Result<TorrentInfo, String> {
    let (root, span) = decode(buf)?;
    let info = root.get("info").ok_or("info yok")?;
    let (s, e) = span.ok_or("info yok")?;
    let info_hash = hex(&Sha1::digest(&buf[s..e]));
    let name = info.get("name.utf-8").or_else(|| info.get("name")).and_then(Bencode::str).ok_or("ad yok")?;
    let mut files = vec![];
    if let Some(Bencode::List(list)) = info.get("files") {
        for f in list {
            let length = f.get("length").and_then(Bencode::int).ok_or("uzunluk yok")? as u64;
            let parts: Vec<String> = match f.get("path.utf-8").or_else(|| f.get("path")) {
                Some(Bencode::List(p)) => p.iter().filter_map(Bencode::str).collect(),
                _ => return Err("yol yok".into()),
            };
            let base = parts.last().cloned().unwrap_or_default();
            let mut path = vec![name.clone()];
            path.extend(parts);
            files.push(TorrentFile { name: base, path: path.join("/"), length });
        }
    } else {
        let length = info.get("length").and_then(Bencode::int).ok_or("uzunluk yok")? as u64;
        files.push(TorrentFile { name: name.clone(), path: name.clone(), length });
    }
    Ok(TorrentInfo {
        info_hash,
        length: files.iter().map(|f| f.length).sum(),
        name,
        files,
        private: info.get("private").and_then(Bencode::int) == Some(1),
        piece_length: info.get("piece length").and_then(Bencode::int).unwrap_or(0) as u64,
    })
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

static BTIH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)[?&]xt=urn:btih:([0-9a-f]{40}|[a-z2-7]{32})(?:&|$)").unwrap());

/// The magnet's btih (40 hex or 32 base32) as lowercase hex, or None.
pub fn magnet_info_hash(magnet: &str) -> Option<String> {
    let v = BTIH.captures(magnet)?.get(1)?.as_str().to_string();
    if v.len() == 40 {
        return Some(v.to_lowercase());
    }
    const ALPHA: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut bits = String::new();
    for c in v.to_uppercase().chars() {
        bits.push_str(&format!("{:05b}", ALPHA.find(c)?));
    }
    Some(bits.as_bytes().chunks(4).map(|b| format!("{:x}", u8::from_str_radix(std::str::from_utf8(b).unwrap(), 2).unwrap())).collect())
}

/// The operating system passes a magnet link as a command-line argument
/// (Windows/Linux) or a URL (macOS). The first valid one (with a btih) wins.
pub fn magnet_from_args<S: AsRef<str>>(args: &[S]) -> Option<String> {
    args.iter().find_map(|a| {
        let v = a.as_ref().trim().trim_matches(|c| c == '"' || c == '\'');
        (v.to_lowercase().starts_with("magnet:?") && magnet_info_hash(v).is_some()).then(|| v.to_string())
    })
}

/// Builds a small .torrent for tests: one folder `name` with the given files
/// (a single-file torrent when the only file is called `name`).
#[cfg(test)]
pub fn make_torrent(name: &str, files: &[(&str, &[u8])], piece_length: usize, url_list: Option<&str>) -> Vec<u8> {
    let b = |s: &str| Bencode::Bytes(s.as_bytes().to_vec());
    let mut data = vec![];
    for (_, body) in files {
        data.extend_from_slice(body);
    }
    let mut pieces = vec![];
    for chunk in data.chunks(piece_length) {
        pieces.extend(Sha1::digest(chunk));
    }
    let mut info = BTreeMap::new();
    info.insert(b"name".to_vec(), b(name));
    info.insert(b"piece length".to_vec(), Bencode::Int(piece_length as i64));
    info.insert(b"pieces".to_vec(), Bencode::Bytes(pieces));
    let list = files
        .iter()
        .map(|(n, body)| {
            let mut f = BTreeMap::new();
            f.insert(b"length".to_vec(), Bencode::Int(body.len() as i64));
            f.insert(b"path".to_vec(), Bencode::List(n.split('/').map(b).collect()));
            Bencode::Dict(f)
        })
        .collect();
    if files.len() == 1 && !files[0].0.contains('/') && files[0].0 == name {
        info.insert(b"length".to_vec(), Bencode::Int(files[0].1.len() as i64));
    } else {
        info.insert(b"files".to_vec(), Bencode::List(list));
    }
    let mut root = BTreeMap::new();
    root.insert(b"info".to_vec(), Bencode::Dict(info));
    if let Some(u) = url_list {
        root.insert(b"url-list".to_vec(), b(u));
    }
    Bencode::Dict(root).encode()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base32(hex: &str) -> String {
        let bits: String = hex.chars().map(|c| format!("{:04b}", c.to_digit(16).unwrap())).collect();
        bits.as_bytes().chunks(5).map(|c| "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567".as_bytes()[usize::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap()] as char).collect()
    }

    #[test]
    fn hex_and_base32_btih_give_the_same_value() {
        let hex = "c12fe1c06bba254a9dc9f519b335aa7c1367a88a";
        assert_eq!(magnet_info_hash(&format!("magnet:?xt=urn:btih:{}&dn=x", hex.to_uppercase())).as_deref(), Some(hex));
        assert_eq!(magnet_info_hash(&format!("magnet:?dn=y&xt=urn:btih:{}", base32(hex))).as_deref(), Some(hex));
        assert_eq!(magnet_info_hash("magnet:?dn=hash-yok"), None);
    }

    #[test]
    fn magnet_is_found_on_the_command_line() {
        let m = "magnet:?xt=urn:btih:c12fe1c06bba254a9dc9f519b335aa7c1367a88a&dn=x";
        assert_eq!(magnet_from_args(&["/usr/bin/compass", "--flag", &format!("\"{m}\"")]).as_deref(), Some(m));
        assert_eq!(magnet_from_args(&["compass", "magnet:?dn=no-hash"]), None);
    }

    #[test]
    fn torrent_round_trip() {
        let t = make_torrent("Film", &[("film.mp4", b"abc"), ("sub/film.srt", b"de")], 16384, None);
        let info = parse_torrent(&t).unwrap();
        assert_eq!(info.name, "Film");
        assert_eq!(info.length, 5);
        assert_eq!(info.files[1].path, "Film/sub/film.srt");
        assert_eq!(info.files[1].name, "film.srt");
        assert_eq!(info.info_hash.len(), 40);
        assert!(parse_torrent(b"d4:infod4:name1:xee").is_err(), "no length");
        assert!(parse_torrent(b"garbage").is_err());
    }
}
