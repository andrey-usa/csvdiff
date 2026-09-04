//! The in-memory half of a comparison.
//!
//! [`RowStore`] owns the parts that must not drift between engines —
//! first-occurrence-wins de-duplication, the duplicate-key report, the full
//! outer join and the sparse cell diffs — so an engine only has to supply
//! normalised rows.
//!
//! The DuckDB engine does all of this in SQL instead and does not use this type.

use std::collections::HashMap;

use crate::columns::{compare_keys, differs, key_of};
use crate::contract::{Cell, CellDiff, ColumnStat, Counts, Section, Val};
use crate::options::Options;

/// Rows shaped as key columns followed by compared columns.
pub struct RowStore {
    key: Vec<String>,
    compared: Vec<String>,
    key_size: usize,
    max_rows: usize,

    /// Key order of first appearance, so the join is deterministic.
    order: Vec<String>,
    first: HashMap<String, Vec<Val>>,
    occurrences: HashMap<String, u32>,
    rows: i64,
}

impl RowStore {
    pub fn new(opt: &Options, compared: &[String]) -> Self {
        RowStore {
            key: opt.key.clone(),
            compared: compared.to_vec(),
            key_size: opt.key.len(),
            max_rows: opt.max_rows,
            order: Vec::new(),
            first: HashMap::new(),
            occurrences: HashMap::new(),
            rows: 0,
        }
    }

    /// Records one already-normalised row.
    pub fn add(&mut self, row: Vec<Val>) {
        self.rows += 1;
        let k = key_of(&row, self.key_size);
        *self.occurrences.entry(k.clone()).or_insert(0) += 1;
        if !self.first.contains_key(&k) {
            self.order.push(k.clone());
            self.first.insert(k, row);
        }
    }

    /// The number of rows read, duplicates included.
    pub fn rows(&self) -> i64 {
        self.rows
    }

    /// The number of distinct composite keys.
    pub fn unique_keys(&self) -> i64 {
        self.occurrences.len() as i64
    }

    /// How many keys appear more than once.
    pub fn duplicate_keys(&self) -> i64 {
        self.occurrences.values().filter(|&&c| c > 1).count() as i64
    }

    /// How many rows carry a duplicated key.
    pub fn duplicate_rows(&self) -> i64 {
        self.occurrences
            .values()
            .filter(|&&c| c > 1)
            .map(|&c| i64::from(c))
            .sum()
    }

    /// The duplicate-key list: most duplicated first, then by key.
    pub fn duplicate_section(&self) -> Section {
        let mut entries: Vec<(&Vec<Val>, u32)> = self
            .order
            .iter()
            .filter_map(|k| {
                let count = self.occurrences[k];
                (count > 1).then(|| (&self.first[k], count))
            })
            .collect();
        entries
            .sort_by(|(x, xn), (y, yn)| yn.cmp(xn).then_with(|| compare_keys(x, y, self.key_size)));

        let truncated = entries.len() > self.max_rows;
        entries.truncate(self.max_rows);
        let rows = entries
            .into_iter()
            .map(|(row, count)| {
                let mut out: Vec<Cell> = row[..self.key_size]
                    .iter()
                    .map(|v| Cell::Value(v.clone()))
                    .collect();
                out.push(Cell::Count(i64::from(count)));
                out
            })
            .collect();

        let mut cols = self.key.clone();
        cols.push("count".to_string());
        Section {
            cols,
            rows,
            truncated,
        }
    }

    /// The compared column names this store was built for.
    pub fn compared(&self) -> &[String] {
        &self.compared
    }
}

/// Everything the full outer join produces, before the sections are capped.
pub struct Joined {
    pub counts: Counts,
    pub columns: Vec<ColumnStat>,
    pub changed: Vec<Vec<Cell>>,
    pub added: Vec<Vec<Cell>>,
    pub removed: Vec<Vec<Cell>>,
    /// A-side rows aligned with `changed`, for `--export-dir`.
    pub changed_a: Vec<Vec<Val>>,
    pub changed_b: Vec<Vec<Val>>,
}

/// The full outer join of two stores on the composite key.
pub fn join(a: &RowStore, b: &RowStore, opt: &Options) -> Joined {
    let key_size = opt.key.len();
    let nc = a.compared.len();

    let mut changed_per = vec![0i64; nc];
    let mut blanked_per = vec![0i64; nc];
    let mut filled_per = vec![0i64; nc];

    let mut changed: Vec<Vec<Cell>> = Vec::new();
    let mut changed_a: Vec<Vec<Val>> = Vec::new();
    let mut changed_b: Vec<Vec<Val>> = Vec::new();
    let mut added: Vec<Vec<Cell>> = Vec::new();
    let mut removed: Vec<Vec<Cell>> = Vec::new();
    let mut matched = 0i64;

    // Iterate in first-appearance order so a run is reproducible.
    for k in &a.order {
        let ar = &a.first[k];
        let Some(br) = b.first.get(k) else {
            removed.push(to_cells(ar));
            continue;
        };
        matched += 1;

        let mut cells: Vec<CellDiff> = Vec::new();
        for i in 0..nc {
            let (x, y) = (&ar[key_size + i], &br[key_size + i]);
            if differs(x, y, opt) {
                cells.push(CellDiff {
                    column: i,
                    a: x.clone(),
                    b: y.clone(),
                });
                changed_per[i] += 1;
                if y.is_none() {
                    blanked_per[i] += 1;
                }
                if x.is_none() {
                    filled_per[i] += 1;
                }
            }
        }
        if !cells.is_empty() {
            let mut row: Vec<Cell> = ar[..key_size]
                .iter()
                .map(|v| Cell::Value(v.clone()))
                .collect();
            row.push(Cell::Diffs(cells));
            changed.push(row);
            changed_a.push(ar.clone());
            changed_b.push(br.clone());
        }
    }
    for k in &b.order {
        if !a.first.contains_key(k) {
            added.push(to_cells(&b.first[k]));
        }
    }

    let columns = (0..nc)
        .map(|i| ColumnStat {
            name: a.compared[i].clone(),
            changed: changed_per[i],
            blanked: blanked_per[i],
            filled: filled_per[i],
        })
        .collect();

    sort_by_key_cells(&mut added, key_size);
    sort_by_key_cells(&mut removed, key_size);
    sort_changed_together(&mut changed, &mut changed_a, &mut changed_b, key_size);

    let counts = Counts {
        a_rows: a.rows(),
        b_rows: b.rows(),
        a_keys: a.unique_keys(),
        b_keys: b.unique_keys(),
        matched,
        unchanged: matched - changed.len() as i64,
        changed: changed.len() as i64,
        added: added.len() as i64,
        removed: removed.len() as i64,
        a_dup_keys: a.duplicate_keys(),
        a_dup_rows: a.duplicate_rows(),
        b_dup_keys: b.duplicate_keys(),
        b_dup_rows: b.duplicate_rows(),
    };

    Joined {
        counts,
        columns,
        changed,
        added,
        removed,
        changed_a,
        changed_b,
    }
}

fn to_cells(row: &[Val]) -> Vec<Cell> {
    row.iter().map(|v| Cell::Value(v.clone())).collect()
}

fn sort_by_key_cells(rows: &mut [Vec<Cell>], key_size: usize) {
    rows.sort_by(|x, y| {
        for i in 0..key_size {
            let ord = match (x[i].value(), y[i].value()) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), None) => std::cmp::Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// Sorts changed rows by key while keeping the parallel A and B row lists in
/// step, which `--export-dir` relies on.
fn sort_changed_together(
    changed: &mut Vec<Vec<Cell>>,
    changed_a: &mut Vec<Vec<Val>>,
    changed_b: &mut Vec<Vec<Val>>,
    key_size: usize,
) {
    let mut idx: Vec<usize> = (0..changed.len()).collect();
    idx.sort_by(|&p, &q| compare_keys(&changed_a[p], &changed_a[q], key_size));

    *changed = reorder(std::mem::take(changed), &idx);
    *changed_a = reorder(std::mem::take(changed_a), &idx);
    *changed_b = reorder(std::mem::take(changed_b), &idx);
}

fn reorder<T>(mut items: Vec<T>, idx: &[usize]) -> Vec<T> {
    let mut slots: Vec<Option<T>> = items.drain(..).map(Some).collect();
    idx.iter()
        .map(|&i| slots[i].take().expect("each index used once"))
        .collect()
}
