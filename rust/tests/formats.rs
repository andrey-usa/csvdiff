//! The three input formats, and the thread count.
//!
//! Both are the same kind of risk: a change that is supposed to be invisible in
//! the answer. A file read as Parquet has to produce the counts it produces as
//! CSV, and a comparison split across four threads has to produce the counts it
//! produces on one — including *which* rows survive `--max-rows`, which is the
//! part that silently stops being true when ranges are merged out of order.

use std::fs;
use std::path::{Path, PathBuf};

use csvdiff::contract::CompareResult;
use csvdiff::engine::compare;
use csvdiff::options::Options;

fn fixture(name: &str) -> PathBuf {
    Path::new("..")
        .join("tests")
        .join("fixtures")
        .join("formats")
        .join(name)
}

fn options(key: &[&str]) -> Options {
    let mut opt = Options::with_key(key.iter().copied());
    opt.engine = "turbo".to_string();
    opt
}

fn run(a: &Path, b: &Path, opt: &mut Options) -> CompareResult {
    compare(a, b, opt)
        .unwrap_or_else(|e| panic!("comparing {} with {}: {e}", a.display(), b.display()))
}

/// Counts and per-column stats, which is what every port is held to.
fn answer(result: &CompareResult) -> String {
    let mut out = format!("{:?}", result.counts);
    for column in &result.columns {
        out.push_str(&format!(
            "\n{} {} {} {}",
            column.name, column.changed, column.blanked, column.filled
        ));
    }
    out
}

/// The same table written eight ways, compared against the same other side.
///
/// Each of these is a *writer* decision — dictionary or plain, five codecs,
/// version-2 delta pages, one row group or five — and none of them is allowed to
/// change the answer. `scripts/make_parquet_fixtures.py` writes them with
/// pyarrow, so this is a test against another implementation's output rather
/// than against our own.
#[test]
fn every_encoding_of_a_parquet_file_reads_the_same_way() {
    let b = fixture("b.csv");
    let expected = answer(&run(&fixture("a.csv"), &b, &mut options(&["id"])));

    for name in [
        "a_dict_snappy.parquet",
        "a_plain_none.parquet",
        "a_dict_gzip.parquet",
        "a_plain_zstd.parquet",
        "a_plain_lz4.parquet",
        "a_delta_v2.parquet",
        "a_dict_row_groups.parquet",
        "a.ndjson",
    ] {
        let got = answer(&run(&fixture(name), &b, &mut options(&["id"])));
        assert_eq!(got, expected, "{name} did not read as the CSV does");
    }
}

/// Either side may be in any format, and the two need not agree: a CSV export
/// compares against the Parquet a warehouse emits.
#[test]
fn the_two_sides_need_not_be_in_the_same_format() {
    let expected = answer(&run(
        &fixture("a.csv"),
        &fixture("b.csv"),
        &mut options(&["id"]),
    ));
    for (a, b) in [
        ("a.ndjson", "b.csv"),
        ("a.csv", "b.ndjson"),
        ("a_dict_snappy.parquet", "b.csv"),
        ("a.csv", "b_dict_snappy.parquet"),
        ("a.ndjson", "b_dict_snappy.parquet"),
        ("a_dict_snappy.parquet", "b_dict_snappy.parquet"),
    ] {
        let got = answer(&run(&fixture(a), &fixture(b), &mut options(&["id"])));
        assert_eq!(got, expected, "{a} against {b}");
    }
}

/// A typed Parquet column has to become the text a CSV of the same data holds,
/// and `typed.csv` is that text: written from the same values by the fixture
/// script, so this compares the reader's rendering against a stated expectation
/// rather than against another copy of itself.
#[test]
fn typed_columns_render_as_the_csv_of_the_same_data() {
    let result = run(
        &fixture("typed.parquet"),
        &fixture("typed.csv"),
        &mut options(&["id"]),
    );
    assert_eq!(
        result.counts.changed,
        0,
        "a typed column rendered differently from the CSV: {:?}",
        result
            .columns
            .iter()
            .filter(|c| c.changed > 0)
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
    );
    assert_eq!(result.counts.matched, 8);
}

/// A file with quoted newlines and doubled quotes, big enough that the engine
/// splits it into chunks.
///
/// This is where threading can actually break: a chunk boundary that lands
/// inside a quoted field would start parsing mid-row, and the only reason it
/// does not is the quote-parity count. The rows are deliberately awkward.
fn awkward_pair(dir: &Path) -> (PathBuf, PathBuf) {
    let mut a = String::from("k,v,w\n");
    let mut b = String::from("k,v,w\n");
    for i in 0..100_000 {
        let quoted = format!("\"line {i}\nsecond, with \"\"quotes\"\" in it\"");
        a.push_str(&format!("K{i},{quoted},{i}\n"));
        let value = if i % 7 == 0 { i + 1 } else { i };
        b.push_str(&format!("K{i},{quoted},{value}\n"));
    }
    let (a_path, b_path) = (dir.join("a.csv"), dir.join("b.csv"));
    fs::write(&a_path, a).expect("the A file");
    fs::write(&b_path, b).expect("the B file");
    (a_path, b_path)
}

#[test]
fn the_thread_count_does_not_change_the_answer() {
    let dir = std::env::temp_dir().join(format!("csvdiff-threads-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("a temp dir");
    let (a, b) = awkward_pair(&dir);
    assert!(
        fs::metadata(&a).expect("the A file").len() > 4 << 20,
        "the fixture must be over the chunking threshold or this tests nothing"
    );

    let mut reference: Option<String> = None;
    for threads in [1usize, 2, 3, 4, 8, 16] {
        let mut opt = options(&["k"]);
        opt.threads = Some(threads);
        opt.max_rows = 25; // small enough that the cap decides which rows are kept
        let result = run(&a, &b, &mut opt);
        let mut got = answer(&result);
        // The kept rows, not just how many: merging ranges out of order would
        // keep a different twenty-five and leave every count intact.
        got.push_str(&format!("\n{:?}", result.changed));
        match &reference {
            None => reference = Some(got),
            Some(want) => assert_eq!(&got, want, "{threads} threads gave a different answer"),
        }
    }
    fs::remove_dir_all(&dir).ok();
}

/// The same, for a columnar file: the rows come from the Parquet reader rather
/// than from a chunked scan, and they are hashed in parallel ranges.
#[test]
fn the_thread_count_does_not_change_a_parquet_answer() {
    let mut reference: Option<String> = None;
    for threads in [1usize, 2, 4, 8] {
        let mut opt = options(&["id"]);
        opt.threads = Some(threads);
        let got = answer(&run(
            &fixture("a_dict_row_groups.parquet"),
            &fixture("b_dict_snappy.parquet"),
            &mut opt,
        ));
        match &reference {
            None => reference = Some(got),
            Some(want) => assert_eq!(&got, want, "{threads} threads gave a different answer"),
        }
    }
}

/// A file that is not what its name says is read as what it is, and a file that
/// is nothing at all is refused by name rather than misread.
#[test]
fn the_format_comes_from_the_bytes_rather_than_the_name() {
    let dir = std::env::temp_dir().join(format!("csvdiff-sniff-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("a temp dir");
    let csv_named_parquet = dir.join("a.parquet");
    fs::copy(fixture("a.csv"), &csv_named_parquet).expect("a copy");
    let expected = answer(&run(
        &fixture("a.csv"),
        &fixture("b.csv"),
        &mut options(&["id"]),
    ));
    let got = answer(&run(
        &csv_named_parquet,
        &fixture("b.csv"),
        &mut options(&["id"]),
    ));
    assert_eq!(got, expected);

    let truncated = dir.join("truncated.parquet");
    let mut bytes = fs::read(fixture("a_dict_snappy.parquet")).expect("the fixture");
    bytes.truncate(bytes.len() / 2);
    fs::write(&truncated, bytes).expect("the truncated file");
    let error = compare(&truncated, &fixture("b.csv"), &mut options(&["id"]))
        .expect_err("half a Parquet file is not readable");
    assert!(
        format!("{error}").contains("truncated"),
        "the error should say what is wrong with the file: {error}"
    );
    fs::remove_dir_all(&dir).ok();
}
