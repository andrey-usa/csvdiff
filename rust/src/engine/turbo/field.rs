//! A field packed into one word, and the SWAR scanning that finds one.
//!
//! Both halves are shared by every reader here — CSV, newline-delimited JSON and
//! Parquet — because they all end up describing a value the same way: a run of
//! bytes somewhere, addressed by offset and length rather than copied into a
//! string. What differs between the formats is where those bytes live and how
//! they are escaped, which is [`super::slab`]'s business, not this module's.

/// A field packed into one word: offset, length, and whether it needs unescaping.
///
/// 40 bits of offset addresses a terabyte and 23 bits of length a field of eight
/// megabytes, which is more than a cell has any business being.
pub(super) type Field = u64;

/// A field that is not there at all: a short row, a missing JSON key, a null.
pub(super) const ABSENT: Field = u64::MAX;
/// A field too long for the packed length. Reported rather than truncated: the
/// length is masked into 23 bits, so silently packing an over-long field would
/// corrupt its value instead of failing.
pub(super) const TOO_LONG: Field = u64::MAX - 1;

const OFFSET_MASK: u64 = (1 << 40) - 1;
const LENGTH_SHIFT: u32 = 40;
const LENGTH_MASK: u64 = (1 << 23) - 1;
const ESCAPED: u64 = 1 << 63;

/// The longest field this packing can address.
pub(super) const MAX_FIELD_LEN: u64 = LENGTH_MASK;

pub(super) fn pack(offset: u64, len: u64, escaped: bool) -> Field {
    if len > MAX_FIELD_LEN {
        return TOO_LONG;
    }
    (offset & OFFSET_MASK)
        | ((len & LENGTH_MASK) << LENGTH_SHIFT)
        | if escaped { ESCAPED } else { 0 }
}

pub(super) fn offset_of(f: Field) -> usize {
    (f & OFFSET_MASK) as usize
}

pub(super) fn len_of(f: Field) -> usize {
    ((f >> LENGTH_SHIFT) & LENGTH_MASK) as usize
}

pub(super) fn is_escaped(f: Field) -> bool {
    f & ESCAPED != 0
}

/// Whether the word describes bytes at all, as opposed to one of the sentinels.
pub(super) fn is_real(f: Field) -> bool {
    f != ABSENT && f != TOO_LONG
}

// ---------------------------------------------------------------------------
// SWAR scanning
// ---------------------------------------------------------------------------

const ONES: u64 = 0x0101_0101_0101_0101;
const HIGH: u64 = 0x8080_8080_8080_8080;

fn broadcast(b: u8) -> u64 {
    (b as u64) * ONES
}

/// Sets the high bit of every byte of `word` that equals `target`.
///
/// `diff - ONES` borrows across a byte only where that byte was zero, and
/// `!diff` cancels the false positives the borrow creates, so what survives
/// marks exactly the matching bytes.
fn match_bits(word: u64, target: u64) -> u64 {
    let diff = word ^ target;
    (diff.wrapping_sub(ONES)) & !diff & HIGH
}

fn word_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().expect("eight bytes"))
}

/// The offset of the first byte at or after `from` that is `a` or `b`, or `end`.
///
/// The slice is cut to `end` first, which is not tidiness: it makes `at + 8 <=
/// data.len()` the loop condition, so the compiler can see that the eight-byte
/// read inside cannot be out of range and drops the bounds check it would
/// otherwise emit on every step of a scan over gigabytes.
pub(super) fn next_of2(data: &[u8], from: usize, end: usize, a: u8, b: u8) -> usize {
    let data = &data[..end.min(data.len())];
    let (ba, bb) = (broadcast(a), broadcast(b));
    let mut at = from;
    while at + 8 <= data.len() {
        let word = word_at(data, at);
        let hits = match_bits(word, ba) | match_bits(word, bb);
        if hits != 0 {
            return at + (hits.trailing_zeros() >> 3) as usize;
        }
        at += 8;
    }
    while at < data.len() {
        if data[at] == a || data[at] == b {
            return at;
        }
        at += 1;
    }
    end
}

/// The offset of the first `target` at or after `from`, or `end`.
pub(super) fn next_of1(data: &[u8], from: usize, end: usize, target: u8) -> usize {
    let data = &data[..end.min(data.len())];
    let bt = broadcast(target);
    let mut at = from;
    while at + 8 <= data.len() {
        let hits = match_bits(word_at(data, at), bt);
        if hits != 0 {
            return at + (hits.trailing_zeros() >> 3) as usize;
        }
        at += 8;
    }
    while at < data.len() {
        if data[at] == target {
            return at;
        }
        at += 1;
    }
    end
}

/// How many `target` bytes there are in `data[from..end]`.
///
/// Only the chunked index build needs this, and only for the quote character:
/// the number of quotes before a position is what says whether that position is
/// inside a quoted field. Counting whole words at a time keeps it far cheaper
/// than the parsing it makes parallel.
pub(super) fn count_byte(data: &[u8], from: usize, end: usize, target: u8) -> usize {
    let data = &data[..end.min(data.len())];
    let bt = broadcast(target);
    let mut n = 0usize;
    let mut at = from;
    while at + 8 <= data.len() {
        n += match_bits(word_at(data, at), bt).count_ones() as usize;
        at += 8;
    }
    while at < data.len() {
        if data[at] == target {
            n += 1;
        }
        at += 1;
    }
    n
}

/// Walks past a quoted field's body, returning the offset after its closing quote.
/// A doubled quote inside is content, not the end.
pub(super) fn skip_quoted(data: &[u8], from: usize, end: usize) -> usize {
    let mut at = from;
    loop {
        let q = next_of1(data, at, end, b'"');
        if q >= end {
            return end;
        }
        if q + 1 < end && data[q + 1] == b'"' {
            at = q + 2;
            continue;
        }
        return q + 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanning_crosses_the_word_boundary() {
        let d = b"aaaaaaaaaaaa,x\nbb";
        assert_eq!(next_of2(d, 0, d.len(), b',', b'\n'), 12);
        assert_eq!(next_of2(d, 13, d.len(), b',', b'\n'), 14);
        assert_eq!(next_of2(d, 15, d.len(), b',', b'\n'), d.len());
        assert_eq!(next_of1(d, 0, d.len(), b'x'), 13);
    }

    #[test]
    fn counting_matches_a_byte_at_a_time() {
        let d = b"a\"b\"\"c\"d\"e\"\"\"f";
        for from in 0..d.len() {
            let want = d[from..].iter().filter(|&&b| b == b'"').count();
            assert_eq!(count_byte(d, from, d.len(), b'"'), want, "from {from}");
        }
    }

    #[test]
    fn a_doubled_quote_is_content() {
        let d = b"\"a\"\"b\",rest";
        assert_eq!(skip_quoted(d, 1, d.len()), 6);
    }

    #[test]
    fn an_over_long_field_is_a_sentinel_rather_than_a_truncation() {
        assert_eq!(pack(0, MAX_FIELD_LEN + 1, false), TOO_LONG);
        let f = pack(1 << 30, MAX_FIELD_LEN, true);
        assert_eq!(offset_of(f), 1 << 30);
        assert_eq!(len_of(f), MAX_FIELD_LEN as usize);
        assert!(is_escaped(f));
    }
}
