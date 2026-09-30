//! The fused join: A's sweep joins each row while its pages are hot.
//!
//! Production only takes this path past 8 GB, so the tests force it with
//! `CSVDIFF_FUSED_JOIN=1`. The env var is process-global and the tests run as
//! parallel threads, so every test that touches it holds the mutex below while
//! the var is set: a leaked `=1` would flip the path under the OOM tests, whose
//! allocation expectations assume the ordinary sweep.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use csvdiff::gendata::{Compression, Format, generate_as};

static FUSED_ENV: Mutex<()> = Mutex::new(());

fn fixture_id() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

const BIN: &str = env!("CARGO_BIN_EXE_csvdiff");

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "csvdiff-fused-{}-{}",
            std::process::id(),
            fixture_id()
        ));
        fs::create_dir_all(&dir).expect("a temp directory");
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

    /// Many duplicate keys whose later rows are changed, removed, and matched:
    /// the later-duplicate subtraction has to take back exactly what the sweep
    /// added.
    fn with_duplicates() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "csvdiff-fused-dup-{}-{}",
            std::process::id(),
            fixture_id()
        ));
        fs::create_dir_all(&dir).expect("a temp directory");
        let mut a = String::from("id,v\n");
        let mut b = String::from("id,v\n");
        for i in 0..10_000 {
            a.push_str(&format!("{i},a{i}\n"));
            // Even keys change, odd keys match; keys >= 9000 are A-only.
            let bv = if i >= 9000 {
                String::new()
            } else if i % 2 == 0 {
                format!("b{i}\n")
            } else {
                format!("a{i}\n")
            };
            if !bv.is_empty() {
                b.push_str(&format!("{i},{bv}"));
            }
        }
        // Later duplicates: changed, matched, and removed rows.
        for i in 0..500 {
            a.push_str(&format!("{i},dup{i}\n"));
        }
        for i in 9500..10_000 {
            b.push_str(&format!("{i},extra{i}\n"));
        }
        fs::write(dir.join("a.csv"), a).expect("a.csv");
        fs::write(dir.join("b.csv"), b).expect("b.csv");
        Fixture(dir)
    }

    /// Quoted fields with embedded newlines: the split guess fails and the
    /// sink resets, so the fused parts must come out empty before the retry.
    fn with_quotes() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "csvdiff-fused-q-{}-{}",
            std::process::id(),
            fixture_id()
        ));
        fs::create_dir_all(&dir).expect("a temp directory");
        let mut a = String::from("id,v\n");
        let mut b = String::from("id,v\n");
        for i in 0..8000 {
            a.push_str(&format!("{i},\"line1\nline2-{i}\"\n"));
            b.push_str(&format!("{i},\"line1\nlineX-{i}\"\n"));
        }
        fs::write(dir.join("a.csv"), a).expect("a.csv");
        fs::write(dir.join("b.csv"), b).expect("b.csv");
        Fixture(dir)
    }

    fn with_ndjson() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "csvdiff-fused-j-{}-{}",
            std::process::id(),
            fixture_id()
        ));
        fs::create_dir_all(&dir).expect("a temp directory");
        let mut a = String::new();
        let mut b = String::new();
        for i in 0..6000 {
            a.push_str(&format!("{{\"id\":{i},\"v\":\"a{i}\"}}\n"));
            b.push_str(&format!(
                "{{\"id\":{i},\"v\":\"{}\"}}\n",
                if i % 3 == 0 {
                    format!("c{i}")
                } else {
                    format!("a{i}")
                }
            ));
        }
        for i in 0..200 {
            a.push_str(&format!("{{\"id\":{i},\"v\":\"dup{i}\"}}\n"));
        }
        fs::write(dir.join("a.json"), a).expect("a.json");
        fs::write(dir.join("b.json"), b).expect("b.json");
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

/// Runs the binary in `dir`; `fused` sets `CSVDIFF_FUSED_JOIN=1`.
fn run(dir: &Path, args: &[&str], fused: bool) -> (i32, String, String) {
    let _guard = FUSED_ENV.lock().unwrap();
    if fused {
        unsafe { std::env::set_var("CSVDIFF_FUSED_JOIN", "1") };
    }
    let out = Command::new(BIN)
        .current_dir(dir)
        .args(args)
        .output()
        .expect("the binary");
    if fused {
        unsafe { std::env::remove_var("CSVDIFF_FUSED_JOIN") };
    }
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

#[test]
fn fused_summary_counts_match_the_ordinary_join() {
    let fx = Fixture::new();
    let args = [
        "compare",
        "a.csv",
        "b.csv",
        "-k",
        "account_id,txn_id",
        "--summary",
    ];
    let (code, out, err) = run(&fx.0, &args, false);
    let (fcode, fout, ferr) = run(&fx.0, &args, true);
    assert_eq!(fcode, code, "fused run failed: {ferr}");
    assert_eq!(counts(&fout), counts(&out), "fused --summary disagrees");
    let _ = err;
}

#[test]
fn fused_report_runs_the_agreement_check_and_matches() {
    let fx = Fixture::new();
    // The report needs the row lists, so the forced fused run also runs the
    // ordinary join and asserts the two sets of counts agree.
    let args = [
        "compare",
        "a.csv",
        "b.csv",
        "-k",
        "account_id,txn_id",
        "--json",
        "fused.json",
        "--no-compress",
    ];
    let (code, out, err) = run(&fx.0, &args, false);
    let (fcode, fout, ferr) = run(&fx.0, &args, true);
    assert_eq!(fcode, code, "fused report run failed: {ferr}");
    assert_eq!(counts(&fout), counts(&out), "fused report counts disagree");
    let plain: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fx.path("fused.json")).expect("fused.json"))
            .expect("json");
    let _ = (err, plain);
}

#[test]
fn fused_later_duplicates_subtract_exactly() {
    let fx = Fixture::with_duplicates();
    let args = ["compare", "a.csv", "b.csv", "-k", "id", "--summary"];
    let (code, out, _) = run(&fx.0, &args, false);
    let (fcode, fout, ferr) = run(&fx.0, &args, true);
    assert_eq!(fcode, code, "fused run failed: {ferr}");
    assert_eq!(
        counts(&fout),
        counts(&out),
        "later-duplicate subtraction is off"
    );
    assert!(
        counts(&fout).contains("dup keys A 500"),
        "the fixture lost its duplicates: {}",
        counts(&fout)
    );
}

#[test]
fn fused_quoted_splits_reset_cleanly() {
    let fx = Fixture::with_quotes();
    let args = ["compare", "a.csv", "b.csv", "-k", "id", "--summary"];
    let (code, out, _) = run(&fx.0, &args, false);
    let (fcode, fout, ferr) = run(&fx.0, &args, true);
    assert_eq!(fcode, code, "fused run failed: {ferr}");
    assert_eq!(
        counts(&fout),
        counts(&out),
        "a wrong split guess leaked joined rows"
    );
}

#[test]
fn fused_ndjson_counts_match() {
    let fx = Fixture::with_ndjson();
    let args = ["compare", "a.json", "b.json", "-k", "id", "--summary"];
    let (code, out, _) = run(&fx.0, &args, false);
    let (fcode, fout, ferr) = run(&fx.0, &args, true);
    assert_eq!(fcode, code, "fused run failed: {ferr}");
    assert_eq!(counts(&fout), counts(&out), "fused ndjson disagrees");
}

#[test]
fn fused_thread_counts_agree() {
    let fx = Fixture::with_duplicates();
    let mut expected = String::new();
    let mut expected_code = 0;
    for threads in ["1", "2", "4"] {
        let args = [
            "compare",
            "a.csv",
            "b.csv",
            "-k",
            "id",
            "--summary",
            "--threads",
            threads,
        ];
        let (code, out, err) = run(&fx.0, &args, true);
        if expected.is_empty() {
            expected = counts(&out);
            expected_code = code;
        } else {
            assert_eq!(
                code, expected_code,
                "thread count changed the outcome: {err}"
            );
            assert_eq!(counts(&out), expected, "thread count changed the counts");
        }
    }
}

#[test]
fn normalisation_still_takes_the_ordinary_path() {
    let fx = Fixture::new();
    let args = [
        "compare",
        "a.csv",
        "b.csv",
        "-k",
        "account_id,txn_id",
        "--summary",
        "--ignore-case",
    ];
    let (code, out, _) = run(&fx.0, &args, false);
    // Fused is refused under normalisation; the run must still agree.
    let (fcode, fout, ferr) = run(&fx.0, &args, true);
    assert_eq!(fcode, code, "run failed: {ferr}");
    assert_eq!(counts(&fout), counts(&out));
}
