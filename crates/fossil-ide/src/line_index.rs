//! UTF-16 ↔ UTF-8 position conversion (rust-analyzer `LineIndex` pattern).
//!
//! # Why this exists
//!
//! LSP positions are **UTF-16 code units** (Monaco / VS Code count columns in
//! UTF-16), but Fossil source is stored and indexed in **UTF-8 bytes** (rowan
//! `TextSize`). Phase 2's [`crate::position`] treated the LSP `character`
//! column as a raw byte offset — correct ONLY for ASCII-only source, and the
//! module explicitly deferred the conversion to Phase 6. RDF IRIs, prefixed
//! names, and comments routinely contain multi-byte characters (`é`, `—`,
//! non-Latin scripts), so any position-bearing feature (hover, goto-def,
//! semantic tokens) points at the wrong column after a multi-byte char without
//! this bridge.
//!
//! # The conversion table (rust-analyzer model)
//!
//! For each line we store the byte positions of every non-ASCII character
//! together with that character's UTF-8 byte length and UTF-16 code-unit
//! length. ASCII-only lines carry an empty table and convert in O(1)
//! (byte-column == UTF-16-column). Lines with multi-byte characters convert by
//! a short scan over the recorded characters — exact in both directions:
//!
//! - **UTF-16 col → byte col** (incoming LSP position → rowan offset).
//! - **byte col → UTF-16 col** (rowan offset → outgoing LSP position).
//!
//! A character whose `utf16_len == 2` is an astral-plane codepoint (a UTF-16
//! surrogate pair, e.g. an emoji); BMP non-ASCII characters have `utf16_len == 1`
//! but `utf8_len` in `2..=3`. Both are handled uniformly.
//!
//! # WASM-clean + FILE-keyed
//!
//! This module is pure (no native deps) and operates over the Salsa-tracked,
//! FILE-keyed [`crate::position::line_offsets`] — it adds **no new per-mapping
//! Salsa query**, so `MAX_PER_MAPPING_FAN_OUT` is untouched. The
//! [`LineIndex`] is built on demand from the file text + the existing
//! line-offset table; callers may build it once per request and reuse it.

/// One non-ASCII character on a line, recorded for UTF-16 ↔ UTF-8 conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct WideChar {
    /// Byte offset of the character WITHIN its line (0-based, from line start).
    byte_col: u32,
    /// UTF-8 length of the character in bytes (`2..=4`).
    utf8_len: u32,
    /// UTF-16 length of the character in code units (`1` for BMP, `2` for
    /// astral-plane / surrogate pairs).
    utf16_len: u32,
}

/// Per-file UTF-16 ↔ UTF-8 conversion table.
///
/// Built from the file text + the FILE-keyed [`crate::position::line_offsets`].
/// Stores, per line, the byte offset of the line start plus the non-ASCII
/// characters on that line (empty for ASCII-only lines). Pure + WASM-clean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of the first character of each line. `line_starts[0] == 0`.
    line_starts: Vec<u32>,
    /// Per line, the non-ASCII characters (parallel to `line_starts`). A line
    /// with no multi-byte characters has an empty inner `Vec` and converts in
    /// O(1).
    wide_chars: Vec<Vec<WideChar>>,
    /// Total byte length of the file (clamps out-of-range offsets).
    len: u32,
}

/// A UTF-16 LSP position (`line`, `character`) — `character` is a UTF-16
/// code-unit column, exactly as the LSP wire protocol defines it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Utf16Position {
    /// 0-based line.
    pub line: u32,
    /// 0-based UTF-16 code-unit column within the line.
    pub character: u32,
}

impl LineIndex {
    /// Build a [`LineIndex`] for `text`, given the FILE-keyed byte-offset
    /// table from [`crate::position::line_offsets`].
    ///
    /// `line_starts[i]` is the byte offset of the first byte of line `i`
    /// (`line_starts[0] == 0`). This is exactly the `Vec<u32>` the Salsa
    /// `line_offsets` query already memoises, so building the [`LineIndex`]
    /// adds no new Salsa query and no per-mapping key.
    #[must_use]
    pub fn new(text: &str, line_starts: &[u32]) -> Self {
        let line_count = line_starts.len();
        let mut wide_chars: Vec<Vec<WideChar>> = vec![Vec::new(); line_count];

        // Scan the text once, attributing each non-ASCII char to its line. We
        // track the current line by advancing through `line_starts` as we pass
        // each line-start byte offset.
        let mut line: usize = 0;
        for (byte_off, ch) in text.char_indices() {
            // Source files we accept are well under u32::MAX bytes.
            let byte_off = u32::try_from(byte_off).unwrap_or(u32::MAX);
            // Advance to the line this byte belongs to.
            while line + 1 < line_count && byte_off >= line_starts[line + 1] {
                line += 1;
            }
            if !ch.is_ascii() {
                let line_start = line_starts[line];
                wide_chars[line].push(WideChar {
                    byte_col: byte_off - line_start,
                    utf8_len: u32::try_from(ch.len_utf8()).unwrap_or(4),
                    utf16_len: u32::try_from(ch.len_utf16()).unwrap_or(2),
                });
            }
        }

        Self {
            line_starts: line_starts.to_vec(),
            wide_chars,
            len: u32::try_from(text.len()).unwrap_or(u32::MAX),
        }
    }

    /// Convert an LSP UTF-16 position to an absolute UTF-8 byte offset.
    ///
    /// Returns `None` if `line` is past the end of the file. A `character`
    /// column past the end of the line clamps to the line's end-of-content
    /// (matching the LSP "position past line end" tolerance).
    #[must_use]
    pub fn offset(&self, pos: Utf16Position) -> Option<u32> {
        let line_start = *self.line_starts.get(pos.line as usize)?;
        let wides = &self.wide_chars[pos.line as usize];

        // Fast path: an ASCII-only line — UTF-16 column == byte column.
        if wides.is_empty() {
            return Some(line_start + pos.character);
        }

        // Walk the line's wide chars, accumulating the UTF-16 column. For each
        // wide char we know how many UTF-16 units precede it; once the target
        // UTF-16 column falls before the next wide char, the remaining columns
        // are ASCII (1 byte == 1 UTF-16 unit).
        let mut byte_col: u32 = 0;
        let mut utf16_col: u32 = 0;
        for wc in wides {
            // ASCII run before this wide char: byte-for-byte == utf16-for-utf16.
            let ascii_run = wc.byte_col - byte_col;
            if pos.character <= utf16_col + ascii_run {
                // Target lands inside the leading ASCII run.
                return Some(line_start + byte_col + (pos.character - utf16_col));
            }
            utf16_col += ascii_run;
            byte_col = wc.byte_col;
            // The wide char itself.
            if pos.character < utf16_col + wc.utf16_len {
                // Target points at (or inside) the wide char — snap to its
                // start byte (no sub-character positions in v0.1).
                return Some(line_start + byte_col);
            }
            utf16_col += wc.utf16_len;
            byte_col += wc.utf8_len;
        }

        // Target is in the trailing ASCII run after the last wide char.
        Some(line_start + byte_col + (pos.character - utf16_col))
    }

    /// Convert an absolute UTF-8 byte offset to an LSP UTF-16 position.
    ///
    /// The inverse of [`Self::offset`]. Clamps an out-of-range `offset` to the
    /// end of the file.
    #[must_use]
    pub fn position(&self, offset: u32) -> Utf16Position {
        let offset = offset.min(self.len);
        // Find the line: the largest `i` with `line_starts[i] <= offset`.
        let line = match self.line_starts.binary_search(&offset) {
            Ok(exact) => exact,
            Err(insert) => insert.saturating_sub(1),
        };
        let line_start = self.line_starts[line];
        let wides = &self.wide_chars[line];
        let target_byte_col = offset - line_start;

        if wides.is_empty() {
            return Utf16Position {
                line: u32::try_from(line).unwrap_or(u32::MAX),
                character: target_byte_col,
            };
        }

        // Accumulate UTF-16 columns up to `target_byte_col`.
        let mut byte_col: u32 = 0;
        let mut utf16_col: u32 = 0;
        for wc in wides {
            if target_byte_col <= wc.byte_col {
                // Inside the ASCII run before this wide char.
                break;
            }
            let ascii_run = wc.byte_col - byte_col;
            utf16_col += ascii_run;
            byte_col = wc.byte_col + wc.utf8_len;
            utf16_col += wc.utf16_len;
        }
        // Add any remaining ASCII bytes between `byte_col` and the target.
        let character = utf16_col + target_byte_col.saturating_sub(byte_col);
        Utf16Position {
            line: u32::try_from(line).unwrap_or(u32::MAX),
            character,
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    /// Compute `line_starts` the same way [`crate::position::line_offsets`]
    /// does (byte offset of the first char of each line, `[0]` always 0).
    fn line_starts(text: &str) -> Vec<u32> {
        let mut offsets = vec![0u32];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                offsets.push(u32::try_from(i + 1).unwrap_or(u32::MAX));
            }
        }
        offsets
    }

    fn index(text: &str) -> LineIndex {
        LineIndex::new(text, &line_starts(text))
    }

    #[test]
    fn ascii_only_roundtrips() {
        let text = "prefix ex: <a>\nUser : ex:Person\n";
        let idx = index(text);
        // Column 7 on line 0 is byte 7 (the `e` of `ex`).
        let pos = Utf16Position {
            line: 0,
            character: 7,
        };
        assert_eq!(idx.offset(pos), Some(7));
        assert_eq!(idx.position(7), pos);
        // Line 1 starts at byte 15.
        let pos2 = Utf16Position {
            line: 1,
            character: 4,
        };
        assert_eq!(idx.offset(pos2), Some(19));
        assert_eq!(idx.position(19), pos2);
    }

    /// A line with a 2-byte (1 UTF-16 unit) char: `é` is 2 bytes UTF-8 but
    /// 1 code unit UTF-16. An identifier AFTER it must map to the right column
    /// in BOTH directions.
    #[test]
    fn multibyte_two_byte_char_column_is_correct() {
        // `# café\n` then an ASCII identifier line.
        //  byte: # =0, ' '=1, c=2, a=3, f=4, é=5..6 (2 bytes), \n=7
        //  utf16:# =0, ' '=1, c=2, a=3, f=4, é=5,         (\n)
        let text = "# café\nname\n";
        let idx = index(text);
        // After `é`: the `\n` is at byte 7, UTF-16 col 6.
        // UTF-16 col 6 on line 0 → byte 7.
        let pos = Utf16Position {
            line: 0,
            character: 6,
        };
        assert_eq!(idx.offset(pos), Some(7));
        // Inverse: byte 7 → UTF-16 col 6.
        assert_eq!(idx.position(7), pos);
        // The char just before `é` (`f` at byte 4 / col 4) is unaffected.
        let f = Utf16Position {
            line: 0,
            character: 4,
        };
        assert_eq!(idx.offset(f), Some(4));
        assert_eq!(idx.position(4), f);
        // Line 1 (`name`) starts at byte 8; col 2 → byte 10.
        let n2 = Utf16Position {
            line: 1,
            character: 2,
        };
        assert_eq!(idx.offset(n2), Some(10));
        assert_eq!(idx.position(10), n2);
    }

    /// An em-dash `—` (U+2014) is 3 bytes UTF-8, 1 UTF-16 code unit. A column
    /// after it on the same line maps correctly.
    #[test]
    fn multibyte_three_byte_em_dash() {
        // `a—b`: a=byte0/col0, —=byte1..3 (3 bytes, 1 utf16), b=byte4/col2.
        let text = "a—b\n";
        let idx = index(text);
        // `b` is at UTF-16 col 2, byte 4.
        let b = Utf16Position {
            line: 0,
            character: 2,
        };
        assert_eq!(idx.offset(b), Some(4));
        assert_eq!(idx.position(4), b);
    }

    /// An astral-plane char (emoji, U+1F600) is 4 bytes UTF-8 and 2 UTF-16
    /// code units (surrogate pair). A column after it maps correctly.
    #[test]
    fn astral_plane_surrogate_pair() {
        // `x😀y`: x=byte0/col0, 😀=byte1..5 (4 bytes, 2 utf16), y=byte5/col3.
        let text = "x😀y\n";
        let idx = index(text);
        let y = Utf16Position {
            line: 0,
            character: 3,
        };
        assert_eq!(idx.offset(y), Some(5));
        assert_eq!(idx.position(5), y);
    }

    #[test]
    fn offset_past_eof_line_is_none() {
        let idx = index("a\nb\n");
        assert!(
            idx.offset(Utf16Position {
                line: 99,
                character: 0
            })
            .is_none()
        );
    }
}
