//! A deterministic pair of CSV files for benchmarking and CI.
//!
//! Both files have 20 columns and share the composite key
//! `(account_id, txn_id)`. File B is file A with a controlled amount of drift,
//! so every run has a known answer:
//!
//! | Drift | Share of rows |
//! |---|---|
//! | `status` changed | 3.0% |
//! | `amount` changed | 1.5% |
//! | `balance` changed | 1.5% |
//! | `value_date` blanked | 0.3% |
//! | `updated_at` changed | 100% (excluded with `--ignore`) |
//! | rows only in B | 0.10% |
//! | rows only in A | 0.10% |
//! | duplicate keys | 0.01% per file |
//!
//! The recipe and the hash are the same as the Python, TypeScript, Java and Go
//! generators — the files come out byte for byte identical — so a benchmark
//! number from any of them is directly comparable.

use std::fmt::Write as _;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::error::{Error, Result};

/// The 20-column schema, in order.
pub const COLUMNS: [&str; 20] = [
    "account_id",
    "txn_id",
    "posting_date",
    "value_date",
    "currency",
    "amount",
    "fee",
    "balance",
    "status",
    "channel",
    "region",
    "branch_code",
    "product_code",
    "counterparty",
    "quantity",
    "rate",
    "category",
    "risk_flag",
    "note",
    "updated_at",
];

const STATUS: [&str; 4] = ["posted", "pending", "settled", "reversed"];
const CHANNEL: [&str; 5] = ["branch", "online", "mobile", "atm", "wire"];
const REGION: [&str; 4] = ["EMEA", "NA", "APAC", "LATAM"];
const CURRENCY: [&str; 4] = ["USD", "EUR", "GBP", "JPY"];
const CATEGORY: [&str; 5] = ["retail", "corporate", "treasury", "cards", "loans"];

// Drift buckets, against a 0..9999 hash bucket per row.
const CHG_STATUS: u64 = 300;
const CHG_AMOUNT: u64 = 150;
const CHG_BALANCE: u64 = 150;
const CHG_VALUE_DATE: u64 = 30;
const REMOVED_MOD: i64 = 1000;
const ADDED_RATIO: i64 = 1000;
const DUP_MOD: i64 = 10_000;

/// splitmix-style hash, matching the other implementations bit for bit.
fn hash(i: i64, salt: i64, seed: i64) -> u64 {
    let mut x = (i.wrapping_mul(31).wrapping_add(salt).wrapping_add(seed)) as u64;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

fn modulo(i: i64, salt: i64, seed: i64, m: u64) -> u64 {
    hash(i, salt, seed) % m
}

/// The 240 dates the generator draws from, starting at 2026-01-01.
fn days() -> Vec<String> {
    // Only 2026 is covered, so a plain civil-date walk is enough and pulls in no
    // calendar library.
    const MONTH_LENGTHS: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut out = Vec::with_capacity(240);
    let (mut month, mut day) = (1u32, 1u32);
    for _ in 0..240 {
        out.push(format!("2026-{month:02}-{day:02}"));
        day += 1;
        if day > MONTH_LENGTHS[(month - 1) as usize] {
            day = 1;
            month += 1;
        }
    }
    out
}

/// Builds one row of one side, appending it to `out` with a trailing newline.
///
/// Money is carried in integer cents and the drift is applied to those integers,
/// never to a float. Every implementation of this generator then produces the
/// same digits without depending on its language's rounding rule — which is
/// what makes the five sets of files byte-identical.
fn row(out: &mut String, i: i64, b_side: bool, seed: i64, days: &[String]) {
    let bucket = modulo(i, 0, seed, 10_000);
    let mut amount_cents = modulo(i, 21, seed, 900_000_000) as i64 - 100_000_000;
    let mut balance_cents = modulo(i, 31, seed, 2_000_000_000) as i64;
    let mut status = STATUS[modulo(i, 11, seed, STATUS.len() as u64) as usize];
    let mut value_date = days[modulo(i, 41, seed, 240) as usize].as_str();

    if b_side {
        if bucket < CHG_STATUS {
            let n = STATUS.len() as u64;
            status = STATUS[((modulo(i, 11, seed, n) + 1) % n) as usize];
        } else if bucket < CHG_STATUS + CHG_AMOUNT {
            amount_cents += 1234;
        } else if bucket < CHG_STATUS + CHG_AMOUNT + CHG_BALANCE {
            // +1%, rounded half up, in cents.
            balance_cents = (balance_cents * 101 + 50) / 100;
        }
        if bucket < CHG_VALUE_DATE {
            value_date = "";
        }
    }

    out.push_str("ACC-");
    pad(out, (i * 7919) % 250_000, 8);
    out.push_str(",TXN-");
    pad(out, i, 11);
    out.push(',');
    out.push_str(&days[modulo(i, 1, seed, 240) as usize]);
    out.push(',');
    out.push_str(value_date);
    out.push(',');
    out.push_str(CURRENCY[modulo(i, 51, seed, 4) as usize]);
    out.push(',');
    money(out, amount_cents);
    out.push(',');
    money(out, modulo(i, 61, seed, 5000) as i64);
    out.push(',');
    money(out, balance_cents);
    out.push(',');
    out.push_str(status);
    out.push(',');
    out.push_str(CHANNEL[modulo(i, 71, seed, 5) as usize]);
    out.push(',');
    out.push_str(REGION[modulo(i, 81, seed, 4) as usize]);
    out.push_str(",BR");
    pad(out, modulo(i, 91, seed, 900) as i64 + 100, 4);
    out.push_str(",P");
    pad(out, modulo(i, 101, seed, 5000) as i64, 5);
    out.push_str(",CP-");
    pad(out, modulo(i, 111, seed, 90_000) as i64, 6);
    out.push(',');
    let _ = write!(out, "{}", modulo(i, 121, seed, 500) + 1);
    out.push_str(",0.");
    pad(out, modulo(i, 131, seed, 1200) as i64, 4);
    out.push(',');
    out.push_str(CATEGORY[modulo(i, 141, seed, 5) as usize]);
    out.push(',');
    out.push(if modulo(i, 151, seed, 20) == 0 {
        'Y'
    } else {
        'N'
    });
    let _ = write!(out, ",batch {} line {},", i % 997 + 1, i % 53 + 1);
    out.push_str(if b_side {
        "2026-09-01 02:15:00"
    } else {
        "2026-08-01 02:15:00"
    });
    out.push('\n');
}

/// Writes an amount held in cents as a two-decimal number.
fn money(out: &mut String, cents: i64) {
    if cents < 0 {
        out.push('-');
    }
    let a = cents.unsigned_abs();
    let _ = write!(out, "{}.{:02}", a / 100, a % 100);
}

fn pad(out: &mut String, v: i64, width: usize) {
    let _ = write!(out, "{v:0width$}");
}

/// Writes both files in one pass.
pub fn generate(rows: i64, a_path: &Path, b_path: &Path, seed: i64) -> Result<()> {
    let days = days();
    let header = COLUMNS.join(",");
    let mut fa = BufWriter::with_capacity(1 << 20, File::create(a_path)?);
    let mut fb = BufWriter::with_capacity(1 << 20, File::create(b_path)?);
    writeln!(fa, "{header}")?;
    writeln!(fb, "{header}")?;

    let dup_extra = (rows / DUP_MOD).max(1);
    let added = (rows / ADDED_RATIO).max(1);

    let mut line = String::with_capacity(256);
    for i in 0..rows {
        line.clear();
        row(&mut line, i, false, seed, &days);
        fa.write_all(line.as_bytes())?;

        if i % REMOVED_MOD != 7 {
            line.clear();
            row(&mut line, i, true, seed, &days);
            fb.write_all(line.as_bytes())?;
        }
        if i % DUP_MOD == 3 && i < rows / 2 {
            line.clear();
            row(&mut line, i, true, seed, &days);
            fb.write_all(line.as_bytes())?;
        }
    }
    for i in 0..dup_extra {
        line.clear();
        row(&mut line, i, false, seed, &days);
        fa.write_all(line.as_bytes())?;
    }
    for i in rows..rows + added {
        line.clear();
        row(&mut line, i, true, seed, &days);
        fb.write_all(line.as_bytes())?;
    }
    fa.flush()?;
    fb.flush()?;
    Ok(())
}

/// Parses `10000`, `10k`, `1m`, `2.5M`.
pub fn parse_rows(s: &str) -> Result<i64> {
    let t: String = s
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| *c != '_' && *c != ',')
        .collect();
    let bad = || Error::new(format!("--rows must be a number, got: {s}"));
    let last = t.chars().last().ok_or_else(bad)?;
    let multiplier: i64 = match last {
        'k' => 1_000,
        'm' => 1_000_000,
        'g' => 1_000_000_000,
        _ => return t.parse().map_err(|_| bad()),
    };
    let value: f64 = t[..t.len() - 1].parse().map_err(|_| bad())?;
    Ok((value * multiplier as f64).round() as i64)
}
