//! Byte offset <-> LSP `Position` conversion.
//!
//! LSP columns are counted in UTF-16 code units by default, while every span
//! we store is a byte offset into the UTF-8 source. All conversion lives here.

use lsp_types::Position;

#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset of the start of each line. Always starts with 0.
    starts: Vec<u32>,
}

impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut starts = Vec::with_capacity(text.len() / 32 + 1);
        starts.push(0);
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                starts.push((i + 1) as u32);
            }
        }
        Self { starts }
    }

    /// Byte offset -> position. Offsets past the end clamp to the end; offsets
    /// inside a multi-byte character round down to the character start.
    pub fn position(&self, text: &str, offset: usize) -> Position {
        let mut offset = offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s as usize <= offset) - 1;
        let line_start = self.starts[line] as usize;
        let character = text[line_start..offset].encode_utf16().count();
        Position::new(line as u32, character as u32)
    }

    /// Position -> byte offset. A column past the end of the line clamps to the
    /// end of the line (excluding `\n` / `\r\n`); a line past the end of the
    /// text clamps to the end of the text. A column pointing into the middle of
    /// a surrogate pair rounds up to the next character.
    pub fn offset(&self, text: &str, pos: Position) -> usize {
        let Some(&start) = self.starts.get(pos.line as usize) else {
            return text.len();
        };
        let start = start as usize;
        let end = self
            .starts
            .get(pos.line as usize + 1)
            .map_or(text.len(), |&e| e as usize);
        let line = &text[start..end];
        let line = line.strip_suffix('\n').unwrap_or(line);
        let line = line.strip_suffix('\r').unwrap_or(line);

        let mut units = 0u32;
        for (i, ch) in line.char_indices() {
            if units >= pos.character {
                return start + i;
            }
            units += ch.len_utf16() as u32;
        }
        start + line.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: u32, character: u32) -> Position {
        Position::new(line, character)
    }

    #[test]
    fn ascii_multiline() {
        let t = "ab\ncde\n\nf";
        let li = LineIndex::new(t);
        assert_eq!(li.position(t, 0), p(0, 0));
        assert_eq!(li.position(t, 2), p(0, 2)); // the '\n' itself
        assert_eq!(li.position(t, 3), p(1, 0));
        assert_eq!(li.position(t, 7), p(2, 0)); // empty line
        assert_eq!(li.position(t, 8), p(3, 0));
        assert_eq!(li.position(t, 9), p(3, 1)); // end of text
        assert_eq!(li.position(t, 999), p(3, 1)); // clamped
    }

    #[test]
    fn utf16_columns() {
        // 'é' = 2 bytes / 1 unit, '😀' = 4 bytes / 2 units, '日' = 3 bytes / 1 unit
        let t = "é😀日x";
        let li = LineIndex::new(t);
        assert_eq!(li.position(t, 0), p(0, 0));
        assert_eq!(li.position(t, 2), p(0, 1));
        assert_eq!(li.position(t, 6), p(0, 3));
        assert_eq!(li.position(t, 9), p(0, 4));
        // mid-character offsets round down
        assert_eq!(li.position(t, 3), p(0, 1));
        assert_eq!(li.offset(t, p(0, 3)), 6);
        assert_eq!(li.offset(t, p(0, 4)), 9);
        // pointing into the middle of a surrogate pair rounds up
        assert_eq!(li.offset(t, p(0, 2)), 6);
    }

    #[test]
    fn crlf_lines_exclude_terminator() {
        let t = "ab\r\ncd";
        let li = LineIndex::new(t);
        assert_eq!(li.position(t, 4), p(1, 0));
        // column beyond the line clamps before the "\r\n"
        assert_eq!(li.offset(t, p(0, 50)), 2);
        assert_eq!(li.offset(t, p(1, 50)), 6);
    }

    #[test]
    fn line_past_end_clamps() {
        let t = "a\nb";
        let li = LineIndex::new(t);
        assert_eq!(li.offset(t, p(9, 0)), t.len());
    }

    #[test]
    fn round_trip_every_char_boundary() {
        let t = "a😀\r\nbé\n\n日本x";
        let li = LineIndex::new(t);
        for (i, _) in t.char_indices() {
            if t[i..].starts_with('\n') && t[..i].ends_with('\r') {
                continue; // the '\n' of CRLF is not addressable
            }
            assert_eq!(li.offset(t, li.position(t, i)), i, "offset {i}");
        }
    }
}
