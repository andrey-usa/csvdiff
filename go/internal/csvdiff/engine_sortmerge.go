package csvdiff

import (
	"errors"
	"fmt"
	"io"
	"os"
	"slices"
	"sort"
)

// compareSortMerge is the out-of-core engine: sort both files by key, then walk
// them together.
//
// The other engines hold at least one whole file in memory, so the largest
// comparison is bounded by the machine. This one is bounded by disk instead.
// Batches of rows are sorted and spilled, the spilled runs are merged back as
// one ordered stream, and the join is a single ordered pass down both sides at
// once -- so memory holds a batch, one row per run, and the capped report, and
// nothing else grows with the file.
//
// That is the classical answer to comparing data larger than memory, and it is
// what reconciliation systems have done since the files lived on tape. It is not
// the fastest engine here and is not meant to be: sorting is O(n log n) where a
// hash join is linear, and the spill writes the data twice more. What it buys is
// that the answer does not depend on how much RAM you have.
//
// Duplicate keys are nearly free, because sorting puts the repeats next to each
// other, and the sections come out in key order for the same reason, so nothing
// is sorted a second time at the end.
func compareSortMerge(aPath, bPath string, opt Options) (EngineResult, error) {
	aHeader, err := readHeader(aPath, opt)
	if err != nil {
		return EngineResult{}, err
	}
	bHeader, err := readHeader(bPath, opt)
	if err != nil {
		return EngineResult{}, err
	}
	resolved, err := ResolveColumns(aHeader, bHeader, opt)
	if err != nil {
		return EngineResult{}, err
	}

	keySize := len(opt.Key)
	width := keySize + len(resolved.Compared)

	work, err := os.MkdirTemp("", "csvdiff-sortmerge-")
	if err != nil {
		return EngineResult{}, err
	}
	defer os.RemoveAll(work)

	aRuns, err := streamInto(work, "a", aPath, aHeader, resolved.Compared, opt, width, keySize)
	if err != nil {
		return EngineResult{}, err
	}
	bRuns, err := streamInto(work, "b", bPath, bHeader, resolved.Compared, opt, width, keySize)
	if err != nil {
		return EngineResult{}, err
	}

	ca, err := aRuns.sorted()
	if err != nil {
		return EngineResult{}, err
	}
	defer ca.close()
	cb, err := bRuns.sorted()
	if err != nil {
		return EngineResult{}, err
	}
	defer cb.close()

	dupA := newDups(opt.Key, opt.MaxRows)
	dupB := newDups(opt.Key, opt.MaxRows)
	joined, err := mergeJoin(ca, cb, opt, resolved.Compared,
		aRuns.rows, bRuns.rows, dupA, dupB, opt.ExportDir != "")
	if err != nil {
		return EngineResult{}, err
	}
	return assemble(resolved.Meta(opt.Key, len(aHeader), len(bHeader)),
		joined, dupA.section(), dupB.section(), opt, resolved.Compared)
}

// streamInto reads a file into the sorter, projecting and normalising a row at a time.
func streamInto(work, name, path string, header, compared []string, opt Options,
	width, keySize int) (*runs, error) {

	dir := work + string(os.PathSeparator) + name
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, err
	}
	f, r, err := newReader(path, opt)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	index := make([]int, width)
	for i, n := range opt.Key {
		index[i] = slices.Index(header, n)
	}
	for i, n := range compared {
		index[keySize+i] = slices.Index(header, n)
	}

	out := newRuns(dir, width, keySize)
	if _, err := r.Read(); err != nil { // header
		return nil, fmt.Errorf("file has no header row: %s", path)
	}
	for {
		rec, err := r.Read()
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, fmt.Errorf("cannot read %s: %w", path, err)
		}
		row := make([]*string, width)
		for i, at := range index {
			if at >= 0 && at < len(rec) {
				row[i] = Normalise(EmptyToNull(rec[at]), opt)
			}
		}
		if err := out.add(row); err != nil {
			return nil, err
		}
	}
	return out, nil
}

// mergeJoin walks two sorted cursors and builds the finished join.
//
// Every other engine indexes a whole file so it can ask "is this key on the
// other side?". This one never asks. Both sides arrive in key order, so the
// smaller key can only be missing from the other file, equal keys are a match,
// and the answer falls out of walking the two cursors forward.
func mergeJoin(a, b cursor, opt Options, compared []string, aRows, bRows int64,
	dupA, dupB *dups, exporting bool) (Joined, error) {

	keySize := len(opt.Key)
	nc := len(compared)

	changedPer := make([]int64, nc)
	blankedPer := make([]int64, nc)
	filledPer := make([]int64, nc)

	changed := newCapped(opt.MaxRows, exporting)
	added := newCapped(opt.MaxRows, exporting)
	removed := newCapped(opt.MaxRows, exporting)
	var changedA, changedB [][]*string

	var matched, aKeys, bKeys int64
	ar, br := a.peek(), b.peek()

	for ar != nil || br != nil {
		var d int
		switch {
		case ar == nil:
			d = 1
		case br == nil:
			d = -1
		default:
			d = compareKey(ar, br, keySize)
		}

		switch {
		case d < 0:
			aKeys++
			removed.add(toAny(ar))
			next, err := skipKey(a, ar, keySize, dupA)
			if err != nil {
				return Joined{}, err
			}
			ar = next
		case d > 0:
			bKeys++
			added.add(toAny(br))
			next, err := skipKey(b, br, keySize, dupB)
			if err != nil {
				return Joined{}, err
			}
			br = next
		default:
			aKeys++
			bKeys++
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
				changed.add(append(row, cells))
				if exporting || len(changedA) <= opt.MaxRows {
					changedA = append(changedA, ar)
					changedB = append(changedB, br)
				}
			}
			nextA, err := skipKey(a, ar, keySize, dupA)
			if err != nil {
				return Joined{}, err
			}
			nextB, err := skipKey(b, br, keySize, dupB)
			if err != nil {
				return Joined{}, err
			}
			ar, br = nextA, nextB
		}
	}

	columns := make([]ColumnStat, nc)
	for i := range nc {
		columns[i] = ColumnStat{Name: compared[i], Changed: changedPer[i],
			Blanked: blankedPer[i], Filled: filledPer[i]}
	}
	counts := Counts{
		ARows: aRows, BRows: bRows, AKeys: aKeys, BKeys: bKeys,
		Matched: matched, Unchanged: matched - changed.total, Changed: changed.total,
		Added: added.total, Removed: removed.total,
		ADupKeys: dupA.keys, ADupRows: dupA.rows,
		BDupKeys: dupB.keys, BDupRows: dupB.rows,
	}
	return Joined{counts, columns, changed.held, added.held, removed.held, changedA, changedB}, nil
}

// skipKey advances past every row sharing the current key, counting the repeats
// as duplicates. The first row of a run is the one the join used, which is
// first-occurrence-wins: the sort is stable and the merge breaks ties by run, so
// file order survives it.
func skipKey(c cursor, current []*string, keySize int, d *dups) ([]*string, error) {
	if err := c.next(); err != nil {
		return nil, err
	}
	repeats := int64(0)
	next := c.peek()
	for next != nil && compareKey(current, next, keySize) == 0 {
		repeats++
		if err := c.next(); err != nil {
			return nil, err
		}
		next = c.peek()
	}
	if repeats > 0 {
		d.record(current, repeats+1)
	}
	return next, nil
}

// capped is a row list that stops growing at the report cap but keeps counting.
//
// The counts in the contract are always exact and the embedded rows are always
// capped, so holding more than the cap only ever serves --export-dir. Not
// holding them is what lets this engine compare a file far larger than memory.
type capped struct {
	held      [][]any
	cap       int
	unbounded bool
	total     int64
}

func newCapped(cap int, unbounded bool) *capped {
	return &capped{cap: cap, unbounded: unbounded}
}

func (c *capped) add(row []any) {
	c.total++
	// One past the cap, so a section can still report that it was truncated.
	if c.unbounded || len(c.held) <= c.cap {
		c.held = append(c.held, row)
	}
}

// dups is the duplicate-key report, kept to the top MaxRows by count.
//
// A bounded list of the worst offenders, rather than every duplicate, so a
// pathological file where every key repeats does not undo the memory bound.
// Ordering matches every other engine: most duplicated first, then by key.
type dups struct {
	keyCols []string
	keySize int
	cap     int
	best    []dupEntry
	keys    int64
	rows    int64
}

type dupEntry struct {
	key   []any
	count int64
	row   []*string
}

func newDups(keyCols []string, cap int) *dups {
	return &dups{keyCols: keyCols, keySize: len(keyCols), cap: cap}
}

func (d *dups) record(row []*string, count int64) {
	d.keys++
	d.rows += count
	key := make([]any, d.keySize)
	for i := range d.keySize {
		key[i] = row[i]
	}
	d.best = append(d.best, dupEntry{key, count, slices.Clone(row)})
	if len(d.best) > d.cap+1 {
		d.sortBest()
		d.best = d.best[:d.cap+1]
	}
}

func (d *dups) sortBest() {
	sort.SliceStable(d.best, func(i, j int) bool {
		if d.best[i].count != d.best[j].count {
			return d.best[i].count > d.best[j].count
		}
		return compareKey(d.best[i].row, d.best[j].row, d.keySize) < 0
	})
}

func (d *dups) section() Section {
	d.sortBest()
	n := min(len(d.best), d.cap)
	rows := make([][]any, 0, n)
	for _, e := range d.best[:n] {
		rows = append(rows, append(slices.Clone(e.key), e.count))
	}
	return Section{
		Cols:      append(append([]string{}, d.keyCols...), "count"),
		Rows:      rows,
		Truncated: d.keys > int64(d.cap),
	}
}
