//! The two text readers: CSV, and newline-delimited JSON.
//!
//! They are one module because they are one design. A JSON value is a contiguous
//! run of bytes in the file exactly as a CSV field is, so a field stays an offset
//! and a length for both and nothing downstream has to know which format it came
//! from. What differs is how a row is walked — by column number, or by key — and
//! how a value is escaped, which [`super::slab`] settles.
//!
//! The two sides of a comparison need not agree: a CSV export compares against
//! the JSON the same pipeline emits, and the key order on each side is free to
//! differ, because the JSON reader joins by name.

use super::field::{ABSENT, Delims, Field, WIDE_SCAN, next_of1, next_of2, pack, skip_quoted};
use super::slab::{Dialect, Slab, text_of};
use crate::error::{Error, Result};
use std::path::Path;

/// Newline-delimited JSON if the first thing that is not whitespace is a brace.
///
/// A CSV header can begin with anything else, and a `{` in the first column of a
/// CSV header is not something this project has ever had to read.
pub(super) fn sniff_dialect(data: &[u8]) -> Dialect {
    for &b in data.iter().take(64) {
        if json_space(b) {
            continue;
        }
        return if b == b'{' {
            Dialect::Json
        } else {
            Dialect::Csv
        };
    }
    Dialect::Csv
}

fn json_space(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\r' || c == b'\n'
}

/// Guesses the delimiter from the header line, defaulting to a comma.
pub(super) fn detect_delimiter(header: &[u8]) -> u8 {
    let mut best = b',';
    let mut best_count = -1i64;
    for c in *b",;\t|" {
        let n = header.iter().filter(|&&b| b == c).count() as i64;
        if n > best_count {
            best = c;
            best_count = n;
        }
    }
    best
}

/// The CSV header row's names, and where the first data row starts.
pub(super) fn csv_header(slab: &Slab, delimiter: u8, path: &Path) -> Result<(Vec<String>, usize)> {
    let data = slab.data();
    let end = data.len();
    if end == 0 {
        return Err(Error::new(format!(
            "file has no header row: {}",
            path.display()
        )));
    }
    let mut names = Vec::new();
    let mut pos = 0usize;
    loop {
        let (field, next) = if pos < end && data[pos] == b'"' {
            let close = skip_quoted(data, pos + 1, end);
            let body_end = close.saturating_sub(1).max(pos + 1);
            (
                quoted_field(data, pos + 1, body_end),
                next_of2(data, close, end, delimiter, b'\n'),
            )
        } else {
            let next = next_of2(data, pos, end, delimiter, b'\n');
            (plain_field(data, pos, next), next)
        };
        names.push(text_of(slab, field));
        if next >= end {
            return Ok((names, end));
        }
        if data[next] == b'\n' {
            return Ok((names, next + 1));
        }
        pos = next + 1;
    }
}

/// A JSON file has no header row, so the column names are the keys of the first
/// object, in the order it lists them.
pub(super) fn json_header(slab: &Slab, path: &Path) -> Result<Vec<String>> {
    let data = slab.data();
    let end = data.len();
    let mut names: Vec<String> = Vec::new();
    let mut pos = 0usize;
    while pos < end && json_space(data[pos]) {
        pos += 1;
    }
    if pos >= end || data[pos] != b'{' {
        return Err(Error::new(format!(
            "file has no JSON object to read: {}",
            path.display()
        )));
    }
    pos += 1;
    loop {
        while pos < end && json_space(data[pos]) {
            pos += 1;
        }
        if pos >= end || data[pos] == b'}' {
            break;
        }
        if data[pos] == b',' {
            pos += 1;
            continue;
        }
        if data[pos] != b'"' {
            break;
        }
        let from = pos + 1;
        let (close, escaped) = skip_json_string(data, pos, end);
        if close <= from {
            break;
        }
        names.push(text_of(
            slab,
            pack(from as u64, (close - 1 - from) as u64, escaped),
        ));
        pos = close;
        while pos < end && json_space(data[pos]) {
            pos += 1;
        }
        if pos >= end || data[pos] != b':' {
            break;
        }
        pos += 1;
        while pos < end && json_space(data[pos]) {
            pos += 1;
        }
        if pos >= end {
            break;
        }
        pos = match data[pos] {
            b'"' => skip_json_string(data, pos, end).0,
            b'{' | b'[' => skip_json_nested(data, pos, end),
            _ => {
                let mut at = pos;
                while at < end && data[at] != b',' && data[at] != b'}' && !json_space(data[at]) {
                    at += 1;
                }
                at
            }
        };
    }
    if names.is_empty() {
        return Err(Error::new(format!(
            "the first JSON object has no keys: {}",
            path.display()
        )));
    }
    Ok(names)
}

/// Skips one JSON string starting at its opening quote, returning the offset one
/// past the closing quote and whether the string holds a backslash.
fn skip_json_string(data: &[u8], mut at: usize, end: usize) -> (usize, bool) {
    at += 1; // the opening quote
    let mut escaped = false;
    loop {
        let stop = next_of2(data, at, end, b'"', b'\\');
        if stop >= end {
            return (end, escaped);
        }
        if data[stop] == b'"' {
            return (stop + 1, escaped);
        }
        escaped = true;
        at = stop + 2; // the backslash and whatever it escapes
        if at > end {
            return (end, escaped);
        }
    }
}

/// Skips a nested object or array, which is not a cell value.
fn skip_json_nested(data: &[u8], mut pos: usize, end: usize) -> usize {
    let mut depth = 0i32;
    while pos < end {
        match data[pos] {
            b'"' => {
                pos = skip_json_string(data, pos, end).0;
                continue;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth -= 1;
                pos += 1;
                if depth <= 0 {
                    return pos;
                }
                continue;
            }
            _ => {}
        }
        pos += 1;
    }
    end
}

/// A quoted CSV field, flagged when it holds a doubled quote. A quote inside the
/// body can only be half of such a pair, which is what makes the test a single
/// scan and lets the unescaping wait until the bytes are actually read.
fn quoted_field(data: &[u8], from: usize, to: usize) -> Field {
    let escaped = next_of1(data, from, to, b'"') < to;
    pack(from as u64, (to - from) as u64, escaped)
}

/// An unquoted field, with a trailing carriage return stripped so CRLF behaves
/// like LF.
fn plain_field(data: &[u8], from: usize, to: usize) -> Field {
    let mut stop = to;
    if stop > from && data[stop - 1] == b'\r' {
        stop -= 1;
    }
    pack(from as u64, (stop - from) as u64, false)
}

/// The offset of the newline ending the row that starts at `pos`.
fn end_of_row(data: &[u8], pos: usize, end: usize) -> usize {
    let mut at = pos;
    while at < end {
        let next = next_of2(data, at, end, b'\n', b'"');
        if next >= end {
            return end;
        }
        if data[next] == b'"' {
            at = skip_quoted(data, next + 1, end);
            continue;
        }
        return next;
    }
    end
}

/// Splits rows into fields, projecting straight to the columns asked for.
///
/// The CSV form is addressed by column number: once the last needed column has
/// been read the rest of the row is skipped to its newline without its fields
/// ever being delimited, so on twenty columns keyed on the first two most of a
/// row is never looked at. The JSON form is addressed by key instead, so it
/// walks the whole object — but only once, and one hash per key rather than a
/// search per wanted column, which at twenty columns would be four hundred
/// comparisons a row.
pub(super) enum RowParser {
    Csv {
        delimiter: u8,
        last_needed: usize,
        /// Which slots each column of the file feeds. Inverted from the wanted
        /// list once, so storing a field is a lookup rather than a walk.
        slots_for: Vec<Vec<u16>>,
        /// The one slot each column feeds, or a sentinel. Almost every column
        /// feeds exactly one, and the hot path stores through this rather than
        /// chasing a pointer into `slots_for` and looping -- per column, per
        /// parse, and a row is parsed about two and a half times: once by the
        /// sweep, then again for each side of the join and once more when a
        /// probe compares keys.
        single: Vec<i32>,
    },
    Json {
        /// The key whose value belongs in each slot; `None` for a column this
        /// file does not have.
        wanted: Vec<Option<String>>,
        /// Open-addressed name to slot, so a key costs one hash.
        slots: Vec<i32>,
        slot_mask: usize,
    },
}

/// `single[column]` when the column feeds no slot, and when it feeds more
/// than one.
const NONE: i32 = -1;
const MANY: i32 = -2;

fn name_hash(s: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &c in s {
        h = (h ^ c as u64).wrapping_mul(0x100_0000_01b3);
    }
    h ^ (h >> 32)
}

impl RowParser {
    pub(super) fn csv(delimiter: u8, source: Vec<Option<usize>>) -> Self {
        let last_needed = source.iter().flatten().copied().max().unwrap_or(0);
        // Inverted once, so storing a field is a lookup rather than a walk of
        // every wanted column: twenty columns against twenty slots is four
        // hundred comparisons a row otherwise. A column can feed more than one
        // slot -- `--compare` may name a key column -- so each entry is a list,
        // and it is one element deep in every case but that one.
        let mut slots_for: Vec<Vec<u16>> = vec![Vec::new(); last_needed + 1];
        for (slot, at) in source.iter().enumerate() {
            if let Some(column) = at {
                slots_for[*column].push(slot as u16);
            }
        }
        // The one-slot shortcut, from the same inversion. NONE is a column no
        // slot wants; MANY is `--compare` naming a key column, the only way a
        // column feeds two, and worth a branch rather than a second array.
        let single = slots_for
            .iter()
            .map(|s| match s.len() {
                0 => NONE,
                1 => i32::from(s[0]),
                _ => MANY,
            })
            .collect();
        RowParser::Csv {
            delimiter,
            last_needed,
            slots_for,
            single,
        }
    }

    pub(super) fn json(wanted: Vec<Option<String>>) -> Self {
        let mut n = 16usize;
        while n < wanted.len() * 4 {
            n <<= 1;
        }
        let mut slots = vec![-1i32; n];
        let slot_mask = n - 1;
        for (i, name) in wanted.iter().enumerate() {
            let Some(name) = name else { continue };
            let mut at = (name_hash(name.as_bytes()) as usize) & slot_mask;
            while slots[at] >= 0 {
                at = (at + 1) & slot_mask;
            }
            slots[at] = i as i32;
        }
        RowParser::Json {
            wanted,
            slots,
            slot_mask,
        }
    }

    /// Parses one row into `out`, returning the offset of the next row.
    ///
    /// A row shorter than the header leaves the missing fields [`ABSENT`], which
    /// compares as absent — a difference to report, not a file to refuse.
    pub(super) fn parse(&self, data: &[u8], start: usize, end: usize, out: &mut [Field]) -> usize {
        match self {
            RowParser::Csv {
                delimiter,
                last_needed,
                slots_for,
                single,
            } => self.parse_csv(
                *delimiter,
                slots_for,
                single,
                *last_needed,
                data,
                start,
                end,
                out,
            ),
            RowParser::Json { .. } => self.parse_json(data, start, end, out),
        }
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn parse_csv(
        &self,
        delimiter: u8,
        slots_for: &[Vec<u16>],
        single: &[i32],
        last_needed: usize,
        data: &[u8],
        start: usize,
        end: usize,
        out: &mut [Field],
    ) -> usize {
        // `out` is reused across rows, so a column this row does not reach has
        // to be blanked or it would show the previous row's value -- but that is
        // only the columns after the row ran out, and a well-formed row runs out
        // of nothing. Clearing all of them up front ran twenty stores per row
        // per file to no purpose.
        let blank_from = |out: &mut [Field], from: usize| {
            if from <= last_needed {
                for slots in &slots_for[from..=last_needed] {
                    for slot in slots {
                        out[*slot as usize] = ABSENT;
                    }
                }
            }
        };
        let mut pos = start;
        let mut column = 0usize;
        // One cursor for the row -- every byte scanned once, both delimiters
        // broadcast once -- but only where a scan step spans several fields. On
        // an eight-byte step it does not, and the plain scan is quicker; see
        // `Delims`.
        let mut delims = Delims::new(data, start, end, delimiter, b'\n');

        while pos <= end {
            let (field, next) = if pos < end && data[pos] == b'"' {
                let close = skip_quoted(data, pos + 1, end);
                let body_end = close.saturating_sub(1).max(pos + 1);
                (quoted_field(data, pos + 1, body_end), delims.next(close))
            } else {
                let next = if WIDE_SCAN {
                    delims.next(pos)
                } else {
                    next_of2(data, pos, end, delimiter, b'\n')
                };
                (plain_field(data, pos, next), next)
            };
            if column <= last_needed {
                let one = single[column];
                if one >= 0 {
                    out[one as usize] = field;
                } else if one == MANY {
                    for slot in &slots_for[column] {
                        out[*slot as usize] = field;
                    }
                }
            }
            column += 1;

            if next >= end {
                blank_from(out, column);
                return end;
            }
            if data[next] == b'\n' {
                blank_from(out, column);
                return next + 1;
            }
            pos = next + 1;
            // Every needed column is filled, so there is nothing to blank: the
            // rest of the row is skipped without being parsed.
            if column > last_needed {
                let eol = end_of_row(data, pos, end);
                return if eol >= end { end } else { eol + 1 };
            }
        }
        blank_from(out, column);
        end
    }

    /// Walks one JSON object, storing the values of the keys we want.
    fn parse_json(&self, data: &[u8], start: usize, end: usize, out: &mut [Field]) -> usize {
        out.fill(ABSENT);
        let mut pos = start;
        while pos < end && json_space(data[pos]) {
            pos += 1;
        }
        if pos >= end {
            return end;
        }
        if data[pos] != b'{' {
            return end_of_json_row(data, pos, end); // not an object: skip the line
        }
        pos += 1;

        loop {
            while pos < end && json_space(data[pos]) {
                pos += 1;
            }
            if pos >= end {
                break;
            }
            match data[pos] {
                b'}' => {
                    pos += 1;
                    break;
                }
                b',' => {
                    pos += 1;
                    continue;
                }
                b'"' => {}
                _ => break, // malformed: stop reading this object
            }
            let key_from = pos + 1;
            let key_end = skip_json_string(data, pos, end).0;
            if key_end > end || key_end < 2 {
                break;
            }
            let key = &data[key_from..key_end - 1];
            pos = key_end;
            while pos < end && json_space(data[pos]) {
                pos += 1;
            }
            if pos >= end || data[pos] != b':' {
                break;
            }
            pos += 1;
            while pos < end && json_space(data[pos]) {
                pos += 1;
            }
            if pos >= end {
                break;
            }

            let field = if data[pos] == b'"' {
                let from = pos + 1;
                let (close, escaped) = skip_json_string(data, pos, end);
                let to = close.saturating_sub(1).max(from);
                pos = close;
                Some(pack(from as u64, (to - from) as u64, escaped))
            } else if data[pos] == b'{' || data[pos] == b'[' {
                // Not a cell value. Left absent rather than guessed at.
                pos = skip_json_nested(data, pos, end);
                None
            } else {
                // A number, true, false or null: it runs to the next comma,
                // brace or space.
                let from = pos;
                while pos < end && data[pos] != b',' && data[pos] != b'}' && !json_space(data[pos])
                {
                    pos += 1;
                }
                if &data[from..pos] == b"null" {
                    None
                } else {
                    Some(pack(from as u64, (pos - from) as u64, false))
                }
            };
            if let Some(field) = field
                && let Some(slot) = self.slot_for(key)
            {
                out[slot] = field;
            }
        }
        end_of_json_row(data, pos, end)
    }

    fn slot_for(&self, key: &[u8]) -> Option<usize> {
        let RowParser::Json {
            wanted,
            slots,
            slot_mask,
        } = self
        else {
            return None;
        };
        let mut at = (name_hash(key) as usize) & slot_mask;
        loop {
            let i = slots[at];
            if i < 0 {
                return None;
            }
            if wanted[i as usize].as_deref().map(str::as_bytes) == Some(key) {
                return Some(i as usize);
            }
            at = (at + 1) & slot_mask;
        }
    }
}

/// Past the end of this object's line. Records are newline-delimited, so a
/// newline outside a string ends the row.
fn end_of_json_row(data: &[u8], mut pos: usize, end: usize) -> usize {
    while pos < end {
        let stop = next_of2(data, pos, end, b'\n', b'"');
        if stop >= end {
            return end;
        }
        if data[stop] == b'\n' {
            return stop + 1;
        }
        let (next, _) = skip_json_string(data, stop, end);
        if next <= stop {
            return end;
        }
        pos = next;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::super::slab::Slab;
    use super::*;

    fn json_slab(text: &str) -> Slab {
        let mut s = Slab::owned(text.as_bytes().to_vec(), Dialect::Json);
        s.set_dialect(Dialect::Json);
        s
    }

    #[test]
    fn a_brace_is_json_and_anything_else_is_csv() {
        assert_eq!(sniff_dialect(b"  {\"a\":1}"), Dialect::Json);
        assert_eq!(sniff_dialect(b"a,b,c\n"), Dialect::Csv);
        assert_eq!(sniff_dialect(b""), Dialect::Csv);
    }

    #[test]
    fn the_header_is_the_first_object_s_keys_in_order() {
        let slab = json_slab("{\"b\":1,\"a\":\"x\",\"n\":null}\n");
        let names = json_header(&slab, Path::new("x")).expect("a header");
        assert_eq!(names, ["b", "a", "n"]);
    }

    #[test]
    fn values_are_read_by_key_whatever_order_they_come_in() {
        let text = "{\"k\":\"1\",\"v\":\"a\"}\n{\"v\":\"b\",\"k\":\"2\"}\n";
        let slab = json_slab(text);
        let parser = RowParser::json(vec![Some("k".into()), Some("v".into())]);
        let mut out = vec![ABSENT; 2];
        let next = parser.parse(slab.data(), 0, text.len(), &mut out);
        assert_eq!(slab.raw(out[0]), b"1");
        assert_eq!(slab.raw(out[1]), b"a");
        parser.parse(slab.data(), next, text.len(), &mut out);
        assert_eq!(slab.raw(out[0]), b"2");
        assert_eq!(slab.raw(out[1]), b"b");
    }

    #[test]
    fn null_a_nested_value_and_a_missing_key_are_all_absent() {
        let text = "{\"k\":\"1\",\"v\":null,\"w\":{\"deep\":1},\"x\":[1,2]}\n";
        let slab = json_slab(text);
        let parser = RowParser::json(vec![
            Some("k".into()),
            Some("v".into()),
            Some("w".into()),
            Some("x".into()),
            Some("missing".into()),
        ]);
        let mut out = vec![0; 5];
        let next = parser.parse(slab.data(), 0, text.len(), &mut out);
        assert_eq!(slab.raw(out[0]), b"1");
        assert_eq!(out[1..], [ABSENT; 4]);
        assert_eq!(next, text.len());
    }

    #[test]
    fn a_newline_inside_a_string_does_not_end_the_row() {
        let text = "{\"k\":\"a\\nb\",\"v\":\"1\"}\n{\"k\":\"c\",\"v\":\"2\"}\n";
        let slab = json_slab(text);
        let parser = RowParser::json(vec![Some("k".into()), Some("v".into())]);
        let mut out = vec![ABSENT; 2];
        let next = parser.parse(slab.data(), 0, text.len(), &mut out);
        assert_eq!(slab.logical(out[0]).collect::<Vec<u8>>(), b"a\nb");
        parser.parse(slab.data(), next, text.len(), &mut out);
        assert_eq!(slab.raw(out[0]), b"c");
    }
}
