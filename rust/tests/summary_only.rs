//! `--summary`: the counts, and nothing on disk.
//!
//! The other three ports in this repository write nothing without an output
//! flag. This one defaults `--out` to `<a>__vs__<b>.html`, so the invocation the
//! benchmark ladder times -- no flags at all -- had this port rendering and
//! gzipping a report while C, C++ and Zig printed a line. `--summary` is the
//! invocation that does what they do.
//!
//! Two things have to hold for it to be worth having, and they pull against
//! each other: it must not write anything, and the counts it prints must be the
//! ones the full run prints. The second is the risk -- the engine skips the
//! sections, and a section is also where its dropped-row total is recorded, so
//! a count computed from the rows that survived rather than from the rows that
//! matched would come out short here and nowhere else.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use csvdiff::gendata::{Compression, Format, generate_as};

const BIN: &str = env!("CARGO_BIN_EXE_csvdiff");

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "csvdiff-summary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("a temp directory");
        // Enough rows that every section has something in it -- changed, added,
        // removed and duplicate keys -- because a section that is empty either
        // way proves nothing about skipping it.
        generate_as(
            20_000,
            &dir.join("a.csv"),
            &dir.join("b.csv"),
            7,
            Format::Csv,
            Compression::None,
        )
        .expect("the generator");
        Fixture(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs in `dir` so a defaulted `--out` lands there and not in the test's cwd.
fn run(dir: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(BIN)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("the binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The counts line without its trailing `| turbo 0.123s`, which is a clock.
fn counts(stdout: &str) -> String {
    let line = stdout.lines().next().unwrap_or_default();
    match line.rfind('|') {
        Some(at) => line[..at].trim_end().to_string(),
        None => line.to_string(),
    }
}

fn files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("the fixture directory")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "a.csv" && n != "b.csv")
        .collect();
    names.sort();
    names
}

#[test]
fn summary_writes_nothing_and_counts_the_same() {
    let fx = Fixture::new();
    let args = ["compare", "a.csv", "b.csv", "-k", "account_id,txn_id"];

    let (full_code, full_out, full_err) = run(&fx.0, &args);
    assert_eq!(full_code, 1, "a run with differences exits 1: {full_err}");
    assert_eq!(
        files(&fx.0),
        vec!["a__vs__b.html".to_string()],
        "the default run writes the defaulted report"
    );

    fs::remove_file(fx.path("a__vs__b.html")).expect("the report");

    let mut summary_args = args.to_vec();
    summary_args.push("--summary");
    let (code, out, err) = run(&fx.0, &summary_args);

    assert_eq!(code, full_code, "--summary changes the exit code: {err}");
    assert!(
        files(&fx.0).is_empty(),
        "--summary wrote {:?}",
        files(&fx.0)
    );
    assert_eq!(
        counts(&out),
        counts(&full_out),
        "--summary and the full run disagree on the counts"
    );
    assert!(
        !out.contains("Report:"),
        "--summary claimed a report it did not write: {out}"
    );
}

#[test]
fn summary_refuses_the_flags_that_ask_for_output() {
    let fx = Fixture::new();
    // Guessing which of the two was meant is how a report silently stops
    // appearing, so each of these is an error rather than a precedence rule.
    for flag in [
        vec!["-o", "r.html"],
        vec!["--out", "r.html"],
        vec!["--json", "r.json"],
        vec!["--export-dir", "."],
    ] {
        let mut args = vec![
            "compare",
            "a.csv",
            "b.csv",
            "-k",
            "account_id,txn_id",
            "--summary",
        ];
        args.extend(flag.iter().copied());
        let (code, _, err) = run(&fx.0, &args);
        assert_eq!(code, 2, "{flag:?} was accepted beside --summary");
        assert!(
            err.contains("--summary writes nothing"),
            "{flag:?}: {}",
            err.trim()
        );
        assert!(files(&fx.0).is_empty(), "{flag:?} wrote {:?}", files(&fx.0));
    }
}
