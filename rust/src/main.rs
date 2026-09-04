//! The command line.
//!
//! ```text
//! csvdiff compare A.csv B.csv --key id,region [--compare c1,c2] [--out report.html]
//! csvdiff compare A.csv B.csv --profile orders
//! ```
//!
//! Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (only
//! with `--fail-on-dups`). That makes it a drop-in CI or pipeline gate.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use csvdiff::error::{Error, Result};
use csvdiff::{Options, compare, parse_list, profiles, render};

const USAGE: &str = "\
usage: csvdiff <command> [options]

commands:
  compare A B     Compare two CSV files on a composite key and write an HTML report

compare options:
  -k, --key COLS          Comma-separated key column(s) (or from --profile)
  -c, --compare COLS      Columns to compare (default: all common non-key columns)
  -i, --ignore COLS       Columns to skip
  -p, --profile NAME      Profile name from csvdiff.toml
      --config PATH       Path to csvdiff.toml
      --trim              Strip whitespace before comparing
      --ignore-case
      --empty-is-null     Treat empty string and null as equal
      --tolerance N       Absolute numeric tolerance
      --max-rows N        Rows embedded per section (default 50000)
      --delimiter D       Force delimiter (default: auto)
      --encoding ENC
      --engine E          auto | duckdb | polars | native
      --threads N
      --memory-limit S    DuckDB memory limit, e.g. 4GB
      --export-dir DIR    Write full changed/added/removed CSVs here
  -o, --out PATH          Report path (default: <a>__vs__<b>.html)
      --json PATH         Also write a JSON summary (counts + column stats) here
      --no-compress       Embed plain JSON instead of gzip (older browsers)
      --fail-on-dups      Exit 3 when either file has duplicate keys

Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (with --fail-on-dups).
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let status = run(&args);
    report_peak_rss();
    ExitCode::from(status)
}

fn run(args: &[String]) -> u8 {
    let Some(command) = args.first() else {
        eprint!("{USAGE}");
        return 2;
    };
    match command.as_str() {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            0
        }
        "compare" => match cmd_compare(&args[1..]) {
            Ok(status) => status,
            Err(e) => {
                eprintln!("error: {e}");
                2
            }
        },
        other => {
            eprint!("error: unknown command: {other}\n\n{USAGE}");
            2
        }
    }
}

/// A small GNU-style argument reader: `--name value`, `--name=value`, `-n value`
/// and bare flags, with everything else treated as a file argument.
struct Args {
    values: Vec<(String, Option<String>)>,
    positional: Vec<String>,
}

/// The options that take no value. Anything else consumes the token after it,
/// which is how a file path can appear before, between or after the options.
const FLAGS: [&str; 7] = [
    "trim",
    "ignore-case",
    "empty-is-null",
    "no-compress",
    "fail-on-dups",
    "help",
    "h",
];

impl Args {
    fn parse(argv: &[String]) -> Result<Self> {
        let mut values = Vec::new();
        let mut positional = Vec::new();
        let mut i = 0;
        while i < argv.len() {
            let token = &argv[i];
            let Some(rest) = token.strip_prefix('-').map(|r| r.trim_start_matches('-')) else {
                positional.push(token.clone());
                i += 1;
                continue;
            };
            if rest.is_empty() {
                positional.push(token.clone());
                i += 1;
                continue;
            }
            match rest.split_once('=') {
                Some((name, value)) => values.push((name.to_string(), Some(value.to_string()))),
                None if FLAGS.contains(&rest) => values.push((rest.to_string(), None)),
                None => {
                    let value = argv
                        .get(i + 1)
                        .ok_or_else(|| Error::new(format!("{token} needs a value")))?;
                    values.push((rest.to_string(), Some(value.clone())));
                    i += 1;
                }
            }
            i += 1;
        }
        Ok(Args { values, positional })
    }

    fn get(&self, long: &str, short: Option<&str>) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == long || Some(name.as_str()) == short)
            .and_then(|(_, value)| value.as_deref())
    }

    fn flag(&self, long: &str) -> bool {
        self.values.iter().any(|(name, _)| name == long)
    }

    fn number<T: std::str::FromStr>(&self, long: &str) -> Result<Option<T>> {
        match self.get(long, None) {
            None => Ok(None),
            Some(v) => v
                .parse::<T>()
                .map(Some)
                .map_err(|_| Error::new(format!("--{long} must be a number, got: {v}"))),
        }
    }
}

fn cmd_compare(argv: &[String]) -> Result<u8> {
    let args = Args::parse(argv)?;
    if args.flag("help") || args.flag("h") {
        print!("{USAGE}");
        return Ok(0);
    }
    if args.positional.len() < 2 {
        return Err(Error::new(
            "compare needs two files: csvdiff compare A.csv B.csv --key ...",
        ));
    }
    let a_path = Path::new(&args.positional[0]);
    let b_path = Path::new(&args.positional[1]);

    let mut opt = Options::default();
    if let Some(name) = args.get("profile", Some("p")) {
        let found = profiles::load(args.get("config", None))?;
        let profile = found
            .get(name)
            .ok_or_else(|| Error::new(format!("profile not found: {name}")))?;
        profile.apply_to(&mut opt);
    }

    // Only options actually given override the profile, hence the `if let`s.
    if let Some(v) = args.get("key", Some("k")) {
        opt.key = parse_list(v);
    }
    if let Some(v) = args.get("compare", Some("c")) {
        opt.compare = Some(parse_list(v));
    }
    if let Some(v) = args.get("ignore", Some("i")) {
        opt.ignore = parse_list(v);
    }
    opt.trim |= args.flag("trim");
    opt.ignore_case |= args.flag("ignore-case");
    opt.empty_is_null |= args.flag("empty-is-null");
    if let Some(v) = args.number::<f64>("tolerance")? {
        opt.tolerance = v;
    }
    if let Some(v) = args.number::<usize>("max-rows")? {
        opt.max_rows = v;
    }
    if let Some(v) = args.number::<usize>("threads")? {
        opt.threads = Some(v);
    }
    if let Some(v) = args.get("delimiter", None) {
        opt.delimiter = v.chars().next();
    }
    if let Some(v) = args.get("encoding", None) {
        opt.encoding = v.to_string();
    }
    if let Some(v) = args.get("engine", None) {
        opt.engine = v.to_string();
    }
    if let Some(v) = args.get("memory-limit", None) {
        opt.memory_limit = Some(v.to_string());
    }
    if let Some(v) = args.get("export-dir", None) {
        opt.export_dir = Some(v.to_string());
    }
    if opt.key.is_empty() {
        return Err(Error::new("--key (or a profile with key) is required"));
    }

    let result = compare(a_path, b_path, &mut opt)?;

    let out_path = match args.get("out", Some("o")) {
        Some(p) => PathBuf::from(p),
        None => PathBuf::from(format!("{}__vs__{}.html", stem(a_path), stem(b_path))),
    };
    let html = render(&result, !args.flag("no-compress"))?;
    fs::write(&out_path, html)
        .map_err(|e| Error::new(format!("cannot write {}: {e}", out_path.display())))?;

    if let Some(path) = args.get("json", None) {
        let summary = serde_json::to_string_pretty(&result.summary())?;
        fs::write(path, summary).map_err(|e| Error::new(format!("cannot write {path}: {e}")))?;
    }

    let c = &result.counts;
    println!(
        "A {} rows | B {} rows | matched {} (changed {}) | added {} | removed {} | \
         dup keys A {} B {} | {} {}s",
        comma(c.a_rows),
        comma(c.b_rows),
        comma(c.matched),
        comma(c.changed),
        comma(c.added),
        comma(c.removed),
        comma(c.a_dup_keys),
        comma(c.b_dup_keys),
        result.meta.engine,
        result.meta.seconds
    );
    if let Ok(meta) = fs::metadata(&out_path) {
        println!(
            "Report: {} ({:.0} KB)",
            out_path.display(),
            meta.len() as f64 / 1024.0
        );
    }

    if args.flag("fail-on-dups") && (c.a_dup_keys > 0 || c.b_dup_keys > 0) {
        return Ok(3);
    }
    Ok(if result.identical() { 0 } else { 1 })
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Groups a count with thousands separators, matching the other implementations.
fn comma(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Prints this process's peak resident set size for the benchmark harness, when
/// it asks.
///
/// Rust has no portable API for it, so this reads `VmHWM` from `/proc`; on a
/// platform without it nothing is printed and the harness records no figure
/// rather than a wrong one.
fn report_peak_rss() {
    if std::env::var_os("CSVDIFF_PRINT_PEAK_RSS").is_none() {
        return;
    }
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let kb: String = rest.chars().filter(char::is_ascii_digit).collect();
            if !kb.is_empty() {
                let _ = writeln!(std::io::stderr(), "PEAK_RSS_KB {kb}");
            }
            return;
        }
    }
}
