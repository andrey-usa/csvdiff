package csvdiff

import (
	"cmp"
	"slices"
)

// RowStore is the in-memory half of a comparison: it owns the parts that must
// not drift between engines — first-occurrence-wins de-duplication, the
// duplicate-key report, the full outer join and the sparse cell diffs — so an
// engine only has to supply normalised rows.
//
// The DuckDB engine does all of this in SQL instead and does not use this type.
type RowStore struct {
	opt      Options
	key      []string
	compared []string
	keySize  int

	order       []string // key order of first appearance, so the join is deterministic
	first       map[string][]*string
	occurrences map[string]int
	rows        int64
}

// NewRowStore prepares a store for rows shaped as key columns then compared columns.
func NewRowStore(opt Options, compared []string) *RowStore {
	return &RowStore{
		opt:         opt,
		key:         opt.Key,
		compared:    compared,
		keySize:     len(opt.Key),
		first:       make(map[string][]*string),
		occurrences: make(map[string]int),
	}
}

// Add records one already-normalised row.
func (s *RowStore) Add(row []*string) {
	s.rows++
	k := KeyOf(row, s.keySize)
	s.occurrences[k]++
	if _, seen := s.first[k]; !seen {
		s.first[k] = row
		s.order = append(s.order, k)
	}
}

// Rows is the number of rows read, duplicates included.
func (s *RowStore) Rows() int64 { return s.rows }

// UniqueKeys is the number of distinct composite keys.
func (s *RowStore) UniqueKeys() int64 { return int64(len(s.occurrences)) }

// DuplicateKeys is how many keys appear more than once.
func (s *RowStore) DuplicateKeys() int64 {
	var n int64
	for _, c := range s.occurrences {
		if c > 1 {
			n++
		}
	}
	return n
}

// DuplicateRows is how many rows carry a duplicated key.
func (s *RowStore) DuplicateRows() int64 {
	var n int64
	for _, c := range s.occurrences {
		if c > 1 {
			n += int64(c)
		}
	}
	return n
}

// DuplicateSection is the duplicate-key list: most duplicated first, then by key.
func (s *RowStore) DuplicateSection() Section {
	type entry struct {
		row []*string
		n   int
	}
	var entries []entry
	for _, k := range s.order {
		if c := s.occurrences[k]; c > 1 {
			entries = append(entries, entry{s.first[k], c})
		}
	}
	slices.SortStableFunc(entries, func(x, y entry) int {
		if c := cmp.Compare(y.n, x.n); c != 0 {
			return c
		}
		return CompareKeys(x.row, y.row, s.keySize)
	})

	limit := min(len(entries), s.opt.MaxRows)
	rows := make([][]any, 0, limit)
	for _, e := range entries[:limit] {
		row := make([]any, 0, s.keySize+1)
		for i := range s.keySize {
			row = append(row, e.row[i])
		}
		rows = append(rows, append(row, e.n))
	}
	return Section{
		Cols:      append(append([]string{}, s.key...), "count"),
		Rows:      rows,
		Truncated: len(entries) > s.opt.MaxRows,
	}
}

// Joined is everything the full outer join produces, before the sections are capped.
type Joined struct {
	Counts   Counts
	Columns  []ColumnStat
	Changed  [][]any
	Added    [][]any
	Removed  [][]any
	ChangedA [][]*string // A-side rows aligned with Changed, for --export-dir
	ChangedB [][]*string
}

// Join is the full outer join of two stores on the composite key.
func Join(a, b *RowStore, opt Options) Joined {
	keySize := len(opt.Key)
	nc := len(a.compared)

	changedPer := make([]int64, nc)
	blankedPer := make([]int64, nc)
	filledPer := make([]int64, nc)

	var changed, added, removed [][]any
	var changedA, changedB [][]*string
	var matched int64

	// Iterate in first-appearance order so a run is reproducible.
	for _, k := range a.order {
		ar := a.first[k]
		br, ok := b.first[k]
		if !ok {
			removed = append(removed, toAny(ar))
			continue
		}
		matched++
		var cells []CellDiff
		for i := range nc {
			x, y := ar[keySize+i], br[keySize+i]
			if Differs(x, y, opt) {
				cells = append(cells, CellDiff{Column: i, A: x, B: y})
				changedPer[i]++
				if y == nil {
					blankedPer[i]++
				}
				if x == nil {
					filledPer[i]++
				}
			}
		}
		if len(cells) > 0 {
			row := make([]any, 0, keySize+1)
			for i := range keySize {
				row = append(row, ar[i])
			}
			changed = append(changed, append(row, cells))
			changedA = append(changedA, ar)
			changedB = append(changedB, br)
		}
	}
	for _, k := range b.order {
		if _, ok := a.first[k]; !ok {
			added = append(added, toAny(b.first[k]))
		}
	}

	columns := make([]ColumnStat, nc)
	for i := range nc {
		columns[i] = ColumnStat{Name: a.compared[i], Changed: changedPer[i], Blanked: blankedPer[i], Filled: filledPer[i]}
	}

	sortRowsByKey(added, keySize)
	sortRowsByKey(removed, keySize)
	sortChangedTogether(changed, changedA, changedB, keySize)

	counts := Counts{
		ARows: a.Rows(), BRows: b.Rows(),
		AKeys: a.UniqueKeys(), BKeys: b.UniqueKeys(),
		Matched: matched, Unchanged: matched - int64(len(changed)), Changed: int64(len(changed)),
		Added: int64(len(added)), Removed: int64(len(removed)),
		ADupKeys: a.DuplicateKeys(), ADupRows: a.DuplicateRows(),
		BDupKeys: b.DuplicateKeys(), BDupRows: b.DuplicateRows(),
	}
	return Joined{counts, columns, changed, added, removed, changedA, changedB}
}

func toAny(row []*string) []any {
	out := make([]any, len(row))
	for i, v := range row {
		out[i] = v
	}
	return out
}

func sortRowsByKey(rows [][]any, keySize int) {
	slices.SortStableFunc(rows, func(x, y []any) int {
		for i := range keySize {
			a, _ := x[i].(*string)
			b, _ := y[i].(*string)
			if a == nil && b == nil {
				continue
			}
			if a == nil {
				return 1
			}
			if b == nil {
				return -1
			}
			if c := cmp.Compare(*a, *b); c != 0 {
				return c
			}
		}
		return 0
	})
}

// sortChangedTogether sorts changed rows by key while keeping the parallel A and
// B row slices in step, which --export-dir relies on.
func sortChangedTogether(changed [][]any, changedA, changedB [][]*string, keySize int) {
	idx := make([]int, len(changed))
	for i := range idx {
		idx[i] = i
	}
	slices.SortStableFunc(idx, func(p, q int) int {
		return CompareKeys(changedA[p], changedA[q], keySize)
	})

	c := make([][]any, len(changed))
	ca := make([][]*string, len(changed))
	cb := make([][]*string, len(changed))
	for n, i := range idx {
		c[n], ca[n], cb[n] = changed[i], changedA[i], changedB[i]
	}
	copy(changed, c)
	copy(changedA, ca)
	copy(changedB, cb)
}

// CapSection caps a row list at --max-rows and records whether anything was cut.
func CapSection(cols []string, rows [][]any, maxRows int) Section {
	if rows == nil {
		rows = [][]any{}
	}
	truncated := len(rows) > maxRows
	return Section{Cols: cols, Rows: rows[:min(len(rows), maxRows)], Truncated: truncated}
}
