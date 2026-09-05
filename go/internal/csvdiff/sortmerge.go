package csvdiff

import (
	"bufio"
	"cmp"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strconv"
)

// External sort: buffer rows until a byte budget is reached, sort that batch,
// spill it, then merge the spilled runs back as one ordered stream.
//
// This is the half of a sort-merge join that keeps memory flat. Nothing here
// holds more than one batch plus one row per open run, so a file ten times
// larger costs ten times the disk and the same memory. Batches are sorted in
// the same order the report sections are written in, so the join downstream
// produces them already sorted and never has to hold a section to sort it.
//
// Ties keep file order. Batches are filled in file order and sorted stably, and
// the merge breaks equal keys by run number, so the first row carrying a key is
// still the first row the join sees -- which is what makes first-occurrence-wins
// mean the same thing here as in every other engine.

// batchBytes is how much memory a batch may occupy before it is spilled.
//
// This is a budget for the batch's real cost, not for the bytes in the file: a
// row arrives as a slice of string pointers, and each of those carries a header
// and its own backing array, so a 180-byte CSV row occupies closer to a
// kilobyte once parsed. Charging only the characters puts several times more in
// the batch than the number suggests.
var batchBytes = defaultBatchBytes()

// defaultBatchBytes is 32 MB unless CSVDIFF_SORTMERGE_BATCH_BYTES overrides it.
// A comparison below the default never spills at all, which would leave the
// interesting half of this file untested.
func defaultBatchBytes() int {
	if v := os.Getenv("CSVDIFF_SORTMERGE_BATCH_BYTES"); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			return n
		}
	}
	return 32 << 20
}

// Per-string and per-row overhead beyond the characters themselves: a string
// header, a pointer, and the row's own slice and backing array.
const (
	stringOverhead = 32
	rowOverhead    = 64
)

// runs accumulates rows, spilling sorted batches to disk when the budget is hit.
type runs struct {
	dir     string
	width   int
	keySize int
	spilled []string
	batch   [][]*string
	held    int
	rows    int64
}

func newRuns(dir string, width, keySize int) *runs {
	return &runs{dir: dir, width: width, keySize: keySize}
}

// add takes one normalised row, spilling the current batch first if it is full.
func (r *runs) add(row []*string) error {
	r.batch = append(r.batch, row)
	r.rows++
	r.held += rowOverhead
	for _, v := range row {
		if v != nil {
			r.held += stringOverhead + len(*v)
		}
	}
	if r.held >= batchBytes {
		return r.spill()
	}
	return nil
}

func (r *runs) sortBatch() {
	slices.SortStableFunc(r.batch, func(x, y []*string) int { return compareKey(x, y, r.keySize) })
}

func (r *runs) spill() error {
	if len(r.batch) == 0 {
		return nil
	}
	r.sortBatch()
	path := filepath.Join(r.dir, fmt.Sprintf("run-%d.bin", len(r.spilled)))
	f, err := os.Create(path)
	if err != nil {
		return err
	}
	w := bufio.NewWriterSize(f, 1<<16)
	for _, row := range r.batch {
		if err := writeRow(w, row); err != nil {
			f.Close()
			return err
		}
	}
	if err := w.Flush(); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	r.spilled = append(r.spilled, path)
	r.batch = nil
	r.held = 0
	return nil
}

// sorted finishes the sort and returns the rows in key order. A sort that never
// spilled is returned straight from memory, so the common case of a file that
// fits pays nothing for the machinery that handles one that does not.
func (r *runs) sorted() (cursor, error) {
	if len(r.spilled) == 0 {
		r.sortBatch()
		c := &sliceCursor{rows: r.batch}
		r.batch = nil
		return c, nil
	}
	if err := r.spill(); err != nil {
		return nil, err
	}
	return newMergeCursor(r.spilled, r.width, r.keySize)
}

// cursor is one ordered pass over sorted rows.
type cursor interface {
	// peek is the row at the cursor, or nil once the rows are exhausted.
	peek() []*string
	// next advances past the current row.
	next() error
	close()
}

type sliceCursor struct {
	rows [][]*string
	at   int
}

func (c *sliceCursor) peek() []*string {
	if c.at < len(c.rows) {
		return c.rows[c.at]
	}
	return nil
}

func (c *sliceCursor) next() error { c.at++; return nil }
func (c *sliceCursor) close()      { c.rows = nil }

// mergeCursor is a k-way merge over the spilled runs, holding one row from each.
type mergeCursor struct {
	files   []*os.File
	readers []*bufio.Reader
	heads   []head
	width   int
	keySize int
}

type head struct {
	row []*string
	run int
}

func newMergeCursor(paths []string, width, keySize int) (*mergeCursor, error) {
	c := &mergeCursor{width: width, keySize: keySize}
	for _, p := range paths {
		f, err := os.Open(p)
		if err != nil {
			c.close()
			return nil, err
		}
		c.files = append(c.files, f)
		c.readers = append(c.readers, bufio.NewReaderSize(f, 1<<16))
	}
	for run := range c.readers {
		if err := c.pull(run); err != nil {
			c.close()
			return nil, err
		}
	}
	return c, nil
}

func (c *mergeCursor) pull(run int) error {
	row, err := readRow(c.readers[run], c.width)
	if err != nil {
		return err
	}
	if row != nil {
		c.heads = append(c.heads, head{row, run})
	}
	return nil
}

// peek finds the smallest head, breaking equal keys by run number so that file
// order survives the merge. A linear scan over one row per run beats a heap at
// the handful of runs a real file produces, and it keeps the tie-break plain.
func (c *mergeCursor) peek() []*string {
	if len(c.heads) == 0 {
		return nil
	}
	best := 0
	for i := 1; i < len(c.heads); i++ {
		d := compareKey(c.heads[i].row, c.heads[best].row, c.keySize)
		if d < 0 || (d == 0 && c.heads[i].run < c.heads[best].run) {
			best = i
		}
	}
	c.heads[0], c.heads[best] = c.heads[best], c.heads[0]
	return c.heads[0].row
}

func (c *mergeCursor) next() error {
	if len(c.heads) == 0 {
		return nil
	}
	run := c.heads[0].run
	c.heads = append(c.heads[:0], c.heads[1:]...)
	return c.pull(run)
}

func (c *mergeCursor) close() {
	for _, f := range c.files {
		f.Close()
	}
	c.files = nil
	c.readers = nil
	c.heads = nil
}

// compareKey orders key values the way the report sections are written: ascending,
// absent values last.
func compareKey(x, y []*string, keySize int) int {
	for i := range keySize {
		a, b := x[i], y[i]
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
}

// The spill format: a length-prefixed field per column, varint lengths, where
// zero is an absent field and a real length is stored one higher.
func writeRow(w *bufio.Writer, row []*string) error {
	var buf [binary.MaxVarintLen64]byte
	for _, v := range row {
		if v == nil {
			if err := w.WriteByte(0); err != nil {
				return err
			}
			continue
		}
		n := binary.PutUvarint(buf[:], uint64(len(*v))+1)
		if _, err := w.Write(buf[:n]); err != nil {
			return err
		}
		if _, err := w.WriteString(*v); err != nil {
			return err
		}
	}
	return nil
}

func readRow(r *bufio.Reader, width int) ([]*string, error) {
	row := make([]*string, width)
	for i := range width {
		n, err := binary.ReadUvarint(r)
		if err != nil {
			if errors.Is(err, io.EOF) && i == 0 {
				return nil, nil
			}
			return nil, fmt.Errorf("spilled run ended mid-row: %w", err)
		}
		if n == 0 {
			continue
		}
		b := make([]byte, n-1)
		if _, err := io.ReadFull(r, b); err != nil {
			return nil, fmt.Errorf("spilled run ended mid-row: %w", err)
		}
		s := string(b)
		row[i] = &s
	}
	return row, nil
}
