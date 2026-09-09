//! Column resolution, normalisation, cell equality and key ordering.
//!
//! These rules are what make the engines interchangeable, so they live in one
//! place rather than in each backend.

use std::cmp::Ordering;

use crate::contract::{EngineMeta, Val};
use crate::error::{Error, Result};
use crate::options::Options;

/// Which columns take part, and which exist on only one side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub compared: Vec<String>,
    pub only_in_a: Vec<String>,
    pub only_in_b: Vec<String>,
}

impl Resolved {
    /// Turns the resolution into the contract's [`EngineMeta`].
    pub fn meta(&self, key: &[String], a_cols: usize, b_cols: usize) -> EngineMeta {
        EngineMeta {
            key: key.to_vec(),
            compared: self.compared.clone(),
            only_in_a: self.only_in_a.clone(),
            only_in_b: self.only_in_b.clone(),
            a_cols,
            b_cols,
        }
    }
}

/// Works out the compared columns from the two headers.
pub fn resolve(a_cols: &[String], b_cols: &[String], opt: &Options) -> Result<Resolved> {
    let missing: Vec<&String> = opt
        .key
        .iter()
        .filter(|k| !a_cols.contains(k) || !b_cols.contains(k))
        .collect();
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
        return Err(Error::new(format!(
            "key column(s) missing from one of the files: {}",
            names.join(", ")
        )));
    }

    // A name that is in neither file is a typo, and a typo here is the one that
    // does not announce itself: `--key` makes the answer impossible and
    // `--compare` refuses, but a misspelled `--ignore` silently compares the
    // column it was meant to drop and reports it as changed on every row. It
    // cost a benchmark run, and a `-i x,y,z` against a `z2` column sat in this
    // repository's own C suite until the check went in.
    //
    // In *neither* file, not in both: `--ignore` is subtractive, so naming a
    // column that only one side has is a real name doing no harm -- it is not
    // compared either way. Only a name nothing has can be a mistake.
    let unknown: Vec<&str> = opt
        .ignore
        .iter()
        .filter(|c| !a_cols.contains(c) && !b_cols.contains(c))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(Error::new(format!(
            "ignore column(s) present in neither file: {}",
            unknown.join(", ")
        )));
    }

    let common: Vec<String> = a_cols
        .iter()
        .filter(|c| b_cols.contains(c))
        .cloned()
        .collect();

    let requested: Vec<String> = match &opt.compare {
        None => common.clone(),
        Some(wanted) => {
            let bad: Vec<&str> = wanted
                .iter()
                .filter(|c| !common.contains(c))
                .map(String::as_str)
                .collect();
            if !bad.is_empty() {
                return Err(Error::new(format!(
                    "compare column(s) not present in both files: {}",
                    bad.join(", ")
                )));
            }
            wanted.clone()
        }
    };

    let compared: Vec<String> = requested
        .into_iter()
        .filter(|c| !opt.key.contains(c) && !opt.ignore.contains(c))
        .collect();

    Ok(Resolved {
        compared,
        only_in_a: a_cols
            .iter()
            .filter(|c| !b_cols.contains(c))
            .cloned()
            .collect(),
        only_in_b: b_cols
            .iter()
            .filter(|c| !a_cols.contains(c))
            .cloned()
            .collect(),
    })
}

/// Treats an empty field as absent, quoted or not.
///
/// This is what DuckDB's reader does, and the other implementations follow it,
/// so every engine here must too or a count would change with the engine.
pub fn empty_to_null(s: &str) -> Val {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Applies `--trim`, `--ignore-case` and `--empty-is-null` to one cell.
pub fn normalise(v: Val, opt: &Options) -> Val {
    let mut s = v?;
    if opt.trim {
        s = s.trim().to_string();
    }
    if opt.ignore_case {
        s = s.to_lowercase();
    }
    if opt.empty_is_null && s.is_empty() {
        return None;
    }
    Some(s)
}

/// Parses a cell as a number, for `--tolerance`.
///
/// Deliberately stricter than [`f64::from_str`]: `inf` and `nan` are ordinary
/// text in a CSV, and treating them as numbers would make two unequal strings
/// compare equal.
fn as_number(v: &Val) -> Option<f64> {
    let s = v.as_deref()?.trim();
    if s.is_empty() {
        return None;
    }
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if !body.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return None;
    }
    if !body
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
    {
        return None;
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// SQL's `IS DISTINCT FROM`, with the numeric tolerance applied where both sides
/// parse as numbers.
///
/// Two absent values are equal; one absent value differs from any present one.
pub fn differs(a: &Val, b: &Val, opt: &Options) -> bool {
    if a.is_none() && b.is_none() {
        return false;
    }
    if opt.tolerance > 0.0
        && let (Some(x), Some(y)) = (as_number(a), as_number(b))
    {
        return (x - y).abs() > opt.tolerance;
    }
    a != b
}

/// Orders key values the way DuckDB orders `VARCHAR`: ascending, with absent
/// values last.
pub fn compare_keys(x: &[Val], y: &[Val], key_size: usize) -> Ordering {
    for i in 0..key_size {
        match (&x[i], &y[i]) {
            (None, None) => continue,
            (None, Some(_)) => return Ordering::Greater,
            (Some(_), None) => return Ordering::Less,
            (Some(a), Some(b)) => match a.cmp(b) {
                Ordering::Equal => continue,
                other => return other,
            },
        }
    }
    Ordering::Equal
}

/// Flattens a composite key into one string for hashing.
///
/// `\x00` marks an absent value and `\x01` separates columns; neither can appear
/// in a CSV field, so distinct keys cannot collide.
pub fn key_of(row: &[Val], key_size: usize) -> String {
    let mut out = String::new();
    for cell in row.iter().take(key_size) {
        match cell {
            None => out.push('\u{0}'),
            Some(s) => out.push_str(s),
        }
        out.push('\u{1}');
    }
    out
}

/// Guesses the delimiter from the header line, defaulting to a comma.
///
/// Ties go to the earliest candidate, so a header with no delimiter at all is a
/// comma — the same choice the other implementations make.
pub fn detect_delimiter(header_line: &str) -> u8 {
    let mut best = b',';
    let mut best_count = 0;
    for candidate in *b",;\t|" {
        let count = header_line.matches(candidate as char).count();
        if count > best_count {
            best = candidate;
            best_count = count;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn opt(key: &[&str], ignore: &[&str]) -> Options {
        Options {
            key: names(key),
            ignore: names(ignore),
            ..Options::default()
        }
    }

    /// The fault this check exists for. `--key` makes the answer impossible and
    /// `--compare` refuses, but a misspelled `--ignore` used to compare the
    /// column it meant to drop and call it changed on every row -- in silence.
    #[test]
    fn an_ignore_name_in_neither_file_is_refused() {
        let a = names(&["x", "y", "id", "z2"]);
        let b = a.clone();
        let err = resolve(&a, &b, &opt(&["id"], &["x", "y", "z"])).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("present in neither file"), "{text}");
        assert!(
            text.contains('z'),
            "the name has to be in the message: {text}"
        );
    }

    /// `--ignore` is subtractive, so a name only one side carries is real and
    /// does no harm: that column is not compared either way. Refusing it would
    /// break the ordinary case of one flag set across files whose schemas
    /// differ at the edges.
    #[test]
    fn an_ignore_name_in_one_file_is_allowed() {
        let a = names(&["id", "shared", "only_a"]);
        let b = names(&["id", "shared", "only_b"]);
        for name in ["only_a", "only_b"] {
            let r = resolve(&a, &b, &opt(&["id"], &[name]));
            assert!(r.is_ok(), "{name} is a real column: {:?}", r.err());
        }
    }

    #[test]
    fn a_real_ignore_name_still_drops_its_column() {
        let a = names(&["id", "keep", "drop"]);
        let b = a.clone();
        let r = resolve(&a, &b, &opt(&["id"], &["drop"])).expect("a real name");
        assert_eq!(r.compared, names(&["keep"]));
    }

    /// The check must not fire on an empty `--ignore`, which is the default.
    #[test]
    fn no_ignore_at_all_is_fine() {
        let a = names(&["id", "v"]);
        let r = resolve(&a, &a.clone(), &opt(&["id"], &[])).expect("no ignore");
        assert_eq!(r.compared, names(&["v"]));
    }

    /// It applies with `--compare` given too: the name is still a typo, even
    /// where the comparison set was named explicitly and `--ignore` is unused.
    #[test]
    fn it_applies_even_when_compare_is_explicit() {
        let a = names(&["id", "v", "w"]);
        let mut o = opt(&["id"], &["nope"]);
        o.compare = Some(names(&["v"]));
        assert!(resolve(&a, &a.clone(), &o).is_err());
    }
}
