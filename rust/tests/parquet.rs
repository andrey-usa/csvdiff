//! The Parquet path against the CSV path, on the same rows.
//!
//! The claim the columnar path has to earn is that it is the *same* comparison:
//! the same data, in a format that stores it as columns of dictionary indices
//! rather than as lines of text, must produce the same report. So each case
//! generates a pair in both formats and asserts the whole result — counts,
//! per-column statistics, every changed cell, every added and removed row, the
//! duplicate sections.
//!
//! The files come from `cpp/build/gen-data`, which writes CSV and Parquet from
//! one field-by-field recipe. Where it has not been built the tests skip by
//! name rather than silently passing: `(cd cpp && make gen-data)`.

use std::path::{Path, PathBuf};
use std::process::Command;

use csvdiff::contract::CompareResult;
use csvdiff::engine::compare;
use csvdiff::options::{Engine, Options};

fn generator() -> Option<PathBuf> {
    // The tests run from `rust/`, so the C++ build is one level up.
    let p = Path::new("../cpp/build/gen-data");
    p.is_file().then(|| p.to_path_buf())
}

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(tool: &Path, rows: &str, extra: &[&str]) -> Option<Self> {
        let dir = std::env::temp_dir().join(format!(
            "csvdiff-pq-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).ok()?;
        for flags in [&[][..], extra] {
            let out = Command::new(tool)
                .args(["--rows", rows, "--out-dir"])
                .arg(&dir)
                .args(["--prefix", "t"])
                .args(flags)
                .output()
                .ok()?;
            assert!(out.status.success(), "gen-data failed: {out:?}");
        }
        Some(Fixture { dir })
    }

    fn path(&self, side: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("t_{side}{ext}"))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn options(key: &[&str], tweak: impl FnOnce(&mut Options)) -> Options {
    let mut opt = Options::with_key(key.iter().copied());
    opt.ignore = vec!["updated_at".to_string()];
    tweak(&mut opt);
    opt
}

/// Everything the contract carries, with the run's own facts removed so two
/// comparisons of the same rows in different formats are directly comparable.
fn contract(r: &CompareResult) -> String {
    let mut v = serde_json::to_value(r).expect("the result serialises");
    // `Meta` serialises flat, so the run's own facts -- which file, how long,
    // which path -- sit inside it and have to come out of there.
    for at in [Some("meta"), None] {
        let target = match at {
            Some(k) => v.get_mut(k),
            None => Some(&mut v),
        };
        if let Some(o) = target.and_then(|t| t.as_object_mut()) {
            for gone in ["a", "b", "seconds", "engine", "generated", "options"] {
                o.remove(gone);
            }
        }
    }
    serde_json::to_string(&v).unwrap()
}

fn run(path_a: &Path, path_b: &Path, mut opt: Options, engine: Engine) -> String {
    opt.engine = engine.to_string();
    let r = compare(path_a, path_b, &mut opt).expect("the comparison runs");
    contract(&r)
}

/// One case: generate CSV and one Parquet flavour, and require the two reports
/// to agree in full.
fn same_report(rows: &str, gen_flags: &[&str], ext: &str, key: &[&str], tweak: fn(&mut Options)) {
    let Some(tool) = generator() else {
        eprintln!("skipping: ../cpp/build/gen-data is not built");
        return;
    };
    let Some(f) = Fixture::new(&tool, rows, gen_flags) else {
        eprintln!("skipping: could not create the fixture");
        return;
    };
    let from_csv = run(
        &f.path("a", ".csv"),
        &f.path("b", ".csv"),
        options(key, tweak),
        Engine::Turbo,
    );
    let from_parquet = run(
        &f.path("a", ext),
        &f.path("b", ext),
        options(key, tweak),
        // The engine asked for is ignored on a Parquet pair; naming turbo here
        // is what proves that.
        Engine::Turbo,
    );
    assert_eq!(
        from_csv, from_parquet,
        "the parquet report differs from the csv one ({rows} rows, {gen_flags:?})"
    );
}

const SNAPPY: &[&str] = &["--format", "parquet", "--compression", "snappy"];
const PLAIN: &[&str] = &["--format", "parquet", "--compression", "none"];

#[test]
fn snappy_matches_the_csv_it_was_written_from() {
    same_report("20k", SNAPPY, ".parquet", &["account_id", "txn_id"], |_| {});
}

#[test]
fn uncompressed_matches() {
    same_report(
        "20k",
        PLAIN,
        ".unc.parquet",
        &["account_id", "txn_id"],
        |_| {},
    );
}

#[test]
fn many_small_row_groups() {
    same_report(
        "20k",
        &[
            "--format",
            "parquet",
            "--compression",
            "snappy",
            "--row-group-size",
            "512",
        ],
        ".parquet",
        &["account_id", "txn_id"],
        |_| {},
    );
}

/// A dictionary budget the data crosses partway makes a column that is
/// dictionary encoded in some row groups and plain in others -- the shape a real
/// writer produces on a high-cardinality string, and the one the reader has to
/// fold into a single form.
#[test]
fn a_column_the_dictionary_gives_up_on() {
    same_report(
        "20k",
        &[
            "--format",
            "parquet",
            "--compression",
            "none",
            "--dict-limit",
            "175",
            "--row-group-size",
            "300",
        ],
        ".unc.parquet",
        &["account_id", "txn_id"],
        |_| {},
    );
}

#[test]
fn every_column_plain() {
    same_report(
        "20k",
        &[
            "--format",
            "parquet",
            "--compression",
            "snappy",
            "--dict-limit",
            "1",
        ],
        ".parquet",
        &["account_id", "txn_id"],
        |_| {},
    );
}

#[test]
fn rows_that_do_not_fill_a_group() {
    same_report(
        "1k",
        PLAIN,
        ".unc.parquet",
        &["account_id", "txn_id"],
        |_| {},
    );
}

/// A key column both sides store as a dictionary takes the shared-id path,
/// where the join never looks at a byte after the dictionaries are mapped.
#[test]
fn a_dictionary_key_column() {
    same_report("20k", SNAPPY, ".parquet", &["currency", "status"], |_| {});
}

#[test]
fn with_trim() {
    same_report("20k", SNAPPY, ".parquet", &["account_id", "txn_id"], |o| {
        o.trim = true;
    });
}

#[test]
fn with_empty_is_null() {
    same_report("20k", SNAPPY, ".parquet", &["account_id", "txn_id"], |o| {
        o.empty_is_null = true;
    });
}

/// A tolerance makes equality non-transitive, so it cannot be given an id: the
/// column falls back to comparing bytes, and must still agree.
#[test]
fn with_tolerance() {
    same_report("20k", SNAPPY, ".parquet", &["account_id", "txn_id"], |o| {
        o.tolerance = 0.01;
    });
}

#[test]
fn with_a_small_cap() {
    same_report("20k", SNAPPY, ".parquet", &["account_id", "txn_id"], |o| {
        o.max_rows = 3;
    });
}

/// One side Parquet and one side text cannot take the columnar path -- there is
/// no column to compare a byte stream against -- so `turbo` decodes the Parquet
/// side into rows instead. Slower than the columnar path, and the same answer:
/// that is what this checks, against csv against csv on the same rows.
#[test]
fn a_mixed_parquet_and_text_pair_is_read_as_rows() {
    let Some(tool) = generator() else {
        eprintln!("skipping: ../cpp/build/gen-data is not built");
        return;
    };
    let Some(f) = Fixture::new(&tool, "1k", SNAPPY) else {
        return;
    };
    let key = &["account_id", "txn_id"];
    let from_text = run(
        &f.path("a", ".csv"),
        &f.path("b", ".csv"),
        options(key, |_| {}),
        Engine::Turbo,
    );
    let mixed = run(
        &f.path("a", ".parquet"),
        &f.path("b", ".csv"),
        options(key, |_| {}),
        Engine::Turbo,
    );
    assert_eq!(
        from_text, mixed,
        "the mixed pair differs from csv against csv"
    );

    // And it is honest about the path it took: `turbo`, not `parquet`.
    let mut opt = options(key, |_| {});
    let result = compare(&f.path("a", ".parquet"), &f.path("b", ".csv"), &mut opt)
        .expect("a mixed pair is answered");
    assert_eq!(result.meta.engine, "turbo");
}

/// An engine that cannot read Parquet says so by name rather than handing the
/// footer to a CSV parser and reporting the parse error it makes of it.
#[test]
fn an_engine_that_cannot_read_parquet_says_so() {
    let Some(tool) = generator() else {
        eprintln!("skipping: ../cpp/build/gen-data is not built");
        return;
    };
    let Some(f) = Fixture::new(&tool, "1k", SNAPPY) else {
        return;
    };
    let mut opt = options(&["account_id", "txn_id"], |_| {});
    opt.engine = "sortmerge".to_string();
    let err = compare(&f.path("a", ".parquet"), &f.path("b", ".csv"), &mut opt)
        .expect_err("sortmerge cannot read parquet");
    assert!(
        err.to_string().contains("cannot read parquet"),
        "the error should say which: {err}"
    );
}

/// The report says which path ran, not which engine was asked for.
#[test]
fn the_report_names_the_parquet_path() {
    let Some(tool) = generator() else {
        eprintln!("skipping: ../cpp/build/gen-data is not built");
        return;
    };
    let Some(f) = Fixture::new(&tool, "1k", SNAPPY) else {
        return;
    };
    let mut opt = options(&["account_id", "txn_id"], |_| {});
    opt.engine = "turbo".to_string();
    let r = compare(&f.path("a", ".parquet"), &f.path("b", ".parquet"), &mut opt).unwrap();
    assert_eq!(r.meta.engine, "parquet");
}
