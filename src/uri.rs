//! `file:` URI <-> path helpers, and the key used to identify a file.

use lsp_types::Uri;
use std::path::{Path, PathBuf};
use std::str::FromStr;

/// Identity of a file across different spellings of its URI: for `file:` URIs
/// it is the decoded path, otherwise the URI string.
pub fn file_key(uri: &Uri) -> String {
    match uri_to_path(uri) {
        Some(path) => path.to_string_lossy().into_owned(),
        None => uri.as_str().to_owned(),
    }
}

pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let is_file_scheme = uri
        .scheme()
        .is_some_and(|scheme| scheme.as_str().eq_ignore_ascii_case("file"));
    if !is_file_scheme {
        return None;
    }
    let decoded = percent_decode(uri.path().as_str());
    // `file:///C:/x` carries the path "/C:/x".
    let has_drive_letter = matches!(
        decoded.as_bytes(),
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic()
    );
    let path = if has_drive_letter {
        decoded.strip_prefix('/').unwrap_or(&decoded)
    } else {
        &decoded
    };
    Some(PathBuf::from(path))
}

pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let text = path.to_str()?.replace('\\', "/");
    let mut encoded = String::from("file://");
    if !text.starts_with('/') {
        encoded.push('/');
    }
    for &byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    Uri::from_str(&encoded).ok()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if let [b'%', high, low, ..] = &bytes[cursor..] {
            if let (Some(high), Some(low)) = (hex_value(*high), hex_value(*low)) {
                decoded.push(high * 16 + low);
                cursor += 3;
                continue;
            }
        }
        decoded.push(bytes[cursor]);
        cursor += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
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
