//! The traps specific to a byte-level engine.
//!
//! `compare.rs` runs turbo alongside every other engine, which covers the
//! ordinary path. What it does not probe is the two ways this design goes wrong
//! quietly, both of which the Java port shipped: a field reaching the hash by
//! one route and the comparison by another, and a key sitting close enough to
//! the end of the file that a wide read cannot be used for it.

use std::fs;
use std::path::PathBuf;

use csvdiff::engine::compare;
use csvdiff::options::{Engine, Options};

struct Fixture {
    dir: PathBuf,
    a: PathBuf,
    b: PathBuf,
}

impl Fixture {
    fn new(a_body: &str, b_body: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "csvdiff-turbo-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        let a = dir.join("a.csv");
        let b = dir.join("b.csv");
        fs::write(&a, a_body).expect("write a");
        fs::write(&b, b_body).expect("write b");
        Fixture { dir, a, b }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Runs one comparison twice — once on turbo, once on the reference engine —
/// and holds them to the same answer.
fn agrees(a_body: &str, b_body: &str, key: &[&str], trim: bool, ignore_case: bool) {
    let f = Fixture::new(a_body, b_body);
    let mut results = Vec::new();
    for engine in [Engine::Native, Engine::Turbo] {
        let mut opt = Options::with_key(key.iter().copied());
        opt.engine = engine.label().to_string();
        opt.trim = trim;
        opt.ignore_case = ignore_case;
        let r = compare(&f.a, &f.b, &mut opt).expect("compare");
        results.push(
            serde_json::json!({
                "counts": r.counts, "columns": r.columns, "changed": r.changed,
                "added": r.added, "removed": r.removed, "dup_a": r.dup_a, "dup_b": r.dup_b,
            })
            .to_string(),
        );
    }
    assert_eq!(results[1], results[0], "turbo diverges from native");
}

/// A key in the last bytes of the file. The Java engine hashed such a key by a
/// different route than one in the middle, so the join missed the last row and
/// reported it as removed from one side and added to the other. Both files here
/// are well formed and hold exactly the keys K1 and K2; only the number of bytes
/// trailing the final key differs.
#[test]
fn a_key_near_the_end_of_the_file_still_matches() {
    agrees(
        "a,k,c\nx,K1,c1\ny,K2,cc\n",
        "a,k,c\nx,K1,c1\ny,K2,cccccccc\n",
        &["k"],
        false,
        false,
    );
}

/// The same, swept across the eight-byte boundary, so the engine cannot be right
/// for one trailing length and wrong for the next.
#[test]
fn the_key_position_in_the_file_never_changes_the_answer() {
    for trailing in 0..=12 {
        let tail = "c".repeat(trailing);
        agrees(
            "a,k,c\nx,K1,c1\ny,K2,z\n",
            &format!("a,k,c\nx,K1,c1\ny,K2,z{tail}\n"),
            &["k"],
            false,
            false,
        );
    }
}

/// Case folding outside ASCII can change a string's length, so an engine that
/// folds bytes for the hash and characters for the comparison puts the two
/// spellings in different buckets. U+212A KELVIN SIGN is three bytes and folds
/// to the one byte "k".
#[test]
fn ignore_case_folds_non_ascii_keys_consistently() {
    agrees(
        "k,v\nCAFÉ,x\nplain,y\n",
        "k,v\ncafé,z\nplain,y\n",
        &["k"],
        false,
        true,
    );
    agrees(
        "k,v\n\u{212a},x\nplain,y\n",
        "k,v\nk,z\nplain,y\n",
        &["k"],
        false,
        true,
    );
}

/// A doubled quote is the one value that is not a slice of the file. This engine
/// flags such a field and drops the second quote of each pair when the bytes are
/// read, so the value never exists anywhere as a contiguous run.
#[test]
fn doubled_quotes_are_unescaped_on_read() {
    agrees(
        "k,v,w\n1,\"a\"\"b\",\n2,\"has,comma\",x\n3,\"two\nlines\",y\n",
        "k,v,w\n1,\"a\"\"b\",z\n2,\"has,comma\",x\n3,\"two\nlines\",y\n",
        &["k"],
        false,
        false,
    );
    // And as a key, where it decides matching rather than just a cell value.
    agrees(
        "k,v\n\"a\"\"b\",one\nplain,two\n",
        "k,v\n\"a\"\"b\",CHANGED\nplain,two\n",
        &["k"],
        false,
        false,
    );
}

/// CRLF, a row shorter than the header, a row longer than it, and a trailing
/// blank line — the shapes a hand-written scanner gets wrong.
#[test]
fn awkward_row_shapes_match_the_reference() {
    agrees(
        "k,v,w\r\n1,x,y\r\n2,p,q\r\n",
        "k,v,w\r\n1,x,CHANGED\r\n2,p,q\r\n",
        &["k"],
        false,
        false,
    );
    agrees(
        "k,v,w\n1,x,y\n2,short\n",
        "k,v,w\n1,x,CHANGED\n2,short\n",
        &["k"],
        false,
        false,
    );
    agrees(
        "k,v,w\n1,x,y\n2,p,q,EXTRA\n",
        "k,v,w\n1,x,CHANGED\n2,p,q,EXTRA\n",
        &["k"],
        false,
        false,
    );
    agrees(
        "k,v,w\n1,x,y\n\n2,p,q\n",
        "k,v,w\n1,x,CHANGED\n\n2,p,q\n",
        &["k"],
        false,
        false,
    );
}

/// A duplicate key joins on its first occurrence, and the repeats are counted
/// rather than joined — the same rule as every other engine.
#[test]
fn duplicate_keys_join_on_the_first_occurrence() {
    agrees(
        "k,v\n1,first\n1,second\n2,b\n",
        "k,v\n1,first\n2,b\n",
        &["k"],
        false,
        false,
    );
}

/// An over-long field in a column the sweep no longer parses.
///
/// The sweep reads each row with a parser configured for the key columns alone,
/// so it never packs the compared ones and cannot see that one of them is past
/// what a field word holds. What catches it instead is arithmetic: a field
/// cannot be longer than the row around it, so a row over the cap is re-read in
/// full. This checks the refusal still happens, and that it happens for a
/// non-key column — the case the cheap check does not cover on its own.
#[test]
fn an_over_long_field_is_still_refused_when_it_is_not_a_key() {
    let big = "x".repeat((1 << 23) + 8); // one field past the twenty-three-bit length
    let a = format!("k,v\nK1,{big}\n");
    let b = format!("k,v\nK1,{big}\n");
    let f = Fixture::new(&a, &b);
    let mut opt = Options::with_key(["k"]);
    opt.engine = Engine::Turbo.label().to_string();
    let err = compare(&f.a, &f.b, &mut opt).expect_err("an over-long field must be refused");
    assert!(
        err.to_string().contains("more than this engine packs"),
        "unexpected error: {err}"
    );
}

/// The same row length, split so that no single field is over the cap. The
/// row-length guard fires here and must then find nothing, because re-reading a
/// long row is only a way to look, not a way to refuse.
#[test]
fn a_long_row_of_short_fields_is_fine() {
    let cell = "x".repeat(1 << 20);
    let row: Vec<String> = (0..12).map(|_| cell.clone()).collect();
    let head: Vec<String> = (0..12).map(|i| format!("c{i}")).collect();
    let body = format!("k,{}\nK1,{}\n", head.join(","), row.join(","));
    let f = Fixture::new(&body, &body);
    let mut opt = Options::with_key(["k"]);
    opt.engine = Engine::Turbo.label().to_string();
    let r = compare(&f.a, &f.b, &mut opt).expect("a long row of short fields is legal");
    assert_eq!(r.counts.matched, 1);
    assert_eq!(r.counts.changed, 0);
}

/// The same, with columns the comparison was told to skip.
fn agrees_ignoring(a_body: &str, b_body: &str, key: &[&str], ignore: &[&str]) {
    let f = Fixture::new(a_body, b_body);
    let mut results = Vec::new();
    for engine in [Engine::Native, Engine::Turbo] {
        let mut opt = Options::with_key(key.iter().copied());
        opt.engine = engine.label().to_string();
        opt.ignore = ignore.iter().map(|s| s.to_string()).collect();
        let r = compare(&f.a, &f.b, &mut opt).expect("compare");
        results.push(
            serde_json::json!({
                "counts": r.counts, "columns": r.columns, "changed": r.changed,
                "added": r.added, "removed": r.removed, "dup_a": r.dup_a, "dup_b": r.dup_b,
            })
            .to_string(),
        );
    }
    assert_eq!(results[1], results[0], "turbo diverges from native");
}

// ---------------------------------------------------------------------------
// The byte-span shortcut: a matched pair whose rows open with the same bytes is
// unchanged without either row being parsed. Everything below is a way for that
// to be true of the bytes and false of the data.
// ---------------------------------------------------------------------------

/// A prefix is not a column. `12,3` opens `12,34`, and a comparison that stopped
/// at the shorter run would call two different rows identical, so the run has to
/// end on a field boundary in both files.
#[test]
fn a_row_that_merely_opens_with_the_other_is_not_the_same_row() {
    agrees(
        "k,v\nK1,3\nK2,x\n",
        "k,v\nK1,34\nK2,x\n",
        &["k"],
        false,
        false,
    );
    // And the other way round, so it cannot be right in one direction only.
    agrees(
        "k,v\nK1,34\nK2,x\n",
        "k,v\nK1,3\nK2,x\n",
        &["k"],
        false,
        false,
    );
}

/// A quoted last column ends on its closing quote, not on the delimiter, so the
/// byte run derived from the field would stop one short and match a row that
/// carries different bytes after it. The shortcut has to decline these.
#[test]
fn a_quoted_last_column_does_not_shorten_the_run() {
    agrees(
        "k,v\nK1,\"a\"\nK2,x\n",
        "k,v\nK1,\"ab\"\nK2,x\n",
        &["k"],
        false,
        false,
    );
    agrees(
        "k,v\nK1,\"a,b\"\nK2,x\n",
        "k,v\nK1,\"a,c\"\nK2,x\n",
        &["k"],
        false,
        false,
    );
}

/// An ignored column inside the run is still inside it, so the bytes differ on a
/// pair that has not changed by the comparison's own definition. Reporting it as
/// changed would be wrong; the shortcut simply does not fire and the columns are
/// compared as before.
#[test]
fn a_column_being_ignored_does_not_make_its_bytes_matter() {
    agrees_ignoring(
        "k,skip,v\nK1,one,x\nK2,a,y\n",
        "k,skip,v\nK1,two,x\nK2,a,y\n",
        &["k"],
        &["skip"],
    );
}

/// Ignored and trailing, which is what the benchmark does: the run stops before
/// it, so rows differing only there take the shortcut and must still be counted
/// unchanged.
#[test]
fn an_ignored_trailing_column_is_outside_the_run() {
    agrees_ignoring(
        "k,v,ts\nK1,x,1\nK2,y,2\n",
        "k,v,ts\nK1,x,9\nK2,y,8\n",
        &["k"],
        &["ts"],
    );
}

/// Two files carrying the same bytes in a different order mean different things.
/// The shortcut is only allowed when both sides read the same columns from the
/// same positions.
#[test]
fn the_same_bytes_in_a_different_column_order_are_not_the_same_row() {
    agrees(
        "k,one,two\nK1,a,b\nK2,p,q\n",
        "k,two,one\nK1,a,b\nK2,p,q\n",
        &["k"],
        false,
        false,
    );
}

/// A row that stops before the last wanted column has no field to end the run
/// at, so there is nothing to compare bytes against.
#[test]
fn a_row_short_of_the_last_column_still_compares() {
    agrees(
        "k,v,w\nK1,x\nK2,y,z\n",
        "k,v,w\nK1,x,w1\nK2,y,z\n",
        &["k"],
        false,
        false,
    );
}

/// An empty last column ends the run at a delimiter with nothing before it,
/// which is a real field and a real boundary.
#[test]
fn an_empty_last_column_is_a_boundary_like_any_other() {
    agrees("k,v\nK1,\nK2,y\n", "k,v\nK1,\nK2,z\n", &["k"], false, false);
    agrees("k,v\nK1,\nK2,y\n", "k,v\nK1,q\nK2,y\n", &["k"], false, false);
}

/// The last row of a file ends at the end of the file rather than at a newline,
/// which is the other way a run can finish.
#[test]
fn a_final_row_without_a_newline_takes_the_run_to_the_end() {
    agrees("k,v\nK1,x\nK2,y", "k,v\nK1,x\nK2,z", &["k"], false, false);
    agrees("k,v\nK1,x\nK2,y", "k,v\nK1,x\nK2,y", &["k"], false, false);
}

/// Normalisation makes rows equal that are not byte-equal, never the reverse, so
/// the shortcut stays sound underneath it -- but only if the slow path still
/// runs for the pairs whose bytes differ.
#[test]
fn trimming_and_folding_still_decide_the_pairs_the_bytes_did_not() {
    agrees("k,v\nK1, x \nK2,y\n", "k,v\nK1,x\nK2,y\n", &["k"], true, false);
    agrees("k,v\nK1,X\nK2,y\n", "k,v\nK1,x\nK2,y\n", &["k"], false, true);
}

/// The case the run's own end has to be checked for, which a boundary in the
/// other file cannot catch. A's last column is quoted and holds a delimiter, so
/// the field ends on its closing quote; B carries the same bytes but never
/// closes the quote, and the byte where A's run stops is a newline in B. The
/// bytes line up and a boundary sits at the end of both, yet the rows differ.
///
/// Only the counts are held to the reference here. The input is malformed, and
/// the two engines already render an unterminated field's value differently --
/// turbo drops the trailing newline the reference keeps -- which is older than
/// this shortcut and not what the test is about. What matters is that the pair
/// is still counted as changed rather than waved through as identical bytes.
#[test]
fn a_run_ending_inside_a_quoted_field_is_not_a_run() {
    let f = Fixture::new("k,v\nK1,\"a,b\"\nK2,x\n", "k,v\nK1,\"a,b\nK2,x\n");
    let mut counts = Vec::new();
    for engine in [Engine::Native, Engine::Turbo] {
        let mut opt = Options::with_key(["k"]);
        opt.engine = engine.label().to_string();
        let r = compare(&f.a, &f.b, &mut opt).expect("compare");
        counts.push(serde_json::json!(r.counts).to_string());
    }
    assert_eq!(counts[1], counts[0], "turbo diverges from native");
    assert!(counts[0].contains("\"changed\":1"), "expected a changed pair");
}
