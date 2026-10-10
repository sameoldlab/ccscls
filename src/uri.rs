//! `file:` URI <-> path helpers, and the key used to identify a file.

use lsp_types::Uri;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Identity of a file across different spellings of its URI: for `file:` URIs
/// it is the decoded path, otherwise the URI string.
pub fn file_key(uri: &Uri) -> String {
    match uri_to_path(uri) {
        Some(p) => p.to_string_lossy().into_owned(),
        None => uri.as_str().to_owned(),
    }
}

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    if !uri.scheme().is_some_and(|s| s.as_str().eq_ignore_ascii_case("file")) {
        return None;
    }
    let decoded = percent_decode(uri.path().as_str());
    // `file:///C:/x` carries the path "/C:/x".
    let b = decoded.as_bytes();
    let path = if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        &decoded[1..]
    } else {
        &decoded[..]
    };
    Some(PathBuf::from(path))
}

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let s = path.to_str()?.replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    Uri::from_str(&out).ok()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_awkward_paths() {
        for p in [
            "/tmp/plain.css",
            "/tmp/with space/a b.css",
            "/tmp/ünï/日本/😀.css",
            "/tmp/100%/a#b?c.css",
        ] {
            let uri = path_to_uri(Path::new(p)).unwrap();
            assert_eq!(uri_to_path(&uri).unwrap(), Path::new(p), "uri = {}", uri.as_str());
        }
    }

    #[test]
    fn different_spellings_share_a_key() {
        let a = Uri::from_str("file:///tmp/a%20b/x.css").unwrap();
        let b = Uri::from_str("file:///tmp/a b/x.css");
        // A raw space is not a valid URI; clients must encode. The encoded form is canonical.
        assert!(b.is_err());
        assert_eq!(file_key(&a), "/tmp/a b/x.css");
    }

    #[test]
    fn windows_drive_letter() {
        let uri = Uri::from_str("file:///C:/proj/a.css").unwrap();
        assert_eq!(uri_to_path(&uri).unwrap(), Path::new("C:/proj/a.css"));
    }

    #[test]
    fn non_file_scheme_has_no_path_and_keys_by_string() {
        let uri = Uri::from_str("untitled:Untitled-1").unwrap();
        assert!(uri_to_path(&uri).is_none());
        assert_eq!(file_key(&uri), "untitled:Untitled-1");
    }

    #[test]
    fn decode_tolerates_malformed_escapes() {
        assert_eq!(percent_decode("a%zzb%4"), "a%zzb%4");
        assert_eq!(percent_decode("%41%"), "A%");
    }
}
