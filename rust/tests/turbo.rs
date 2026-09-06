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
