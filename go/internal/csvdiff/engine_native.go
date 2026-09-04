package csvdiff

import (
	"bufio"
	"encoding/csv"
	"errors"
	"fmt"
	"io"
	"os"
	"slices"
	"strings"
)

// compareNative parses with encoding/csv and joins in a map.
//
// Both files are held in memory, so this is for data that fits comfortably in
// RAM. It is the fallback when DuckDB cannot load, and the baseline the other
// engine is measured against.
func compareNative(aPath, bPath string, opt Options) (EngineResult, error) {
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

	a, err := readStore(aPath, aHeader, resolved.Compared, opt)
	if err != nil {
		return EngineResult{}, err
	}
	b, err := readStore(bPath, bHeader, resolved.Compared, opt)
	if err != nil {
		return EngineResult{}, err
	}

	dupA, dupB := a.DuplicateSection(), b.DuplicateSection()
	joined := Join(a, b, opt)
	return assemble(resolved.Meta(opt.Key, len(aHeader), len(bHeader)), joined, dupA, dupB, opt, resolved.Compared)
}

// newReader opens a CSV reader with the delimiter forced or sniffed from the header.
func newReader(path string, opt Options) (*os.File, *csv.Reader, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, nil, fmt.Errorf("cannot read %s: %w", path, err)
	}
	delim := opt.DelimiterRune()
	if delim == 0 {
		line, err := bufio.NewReader(f).ReadString('\n')
		if err != nil && !errors.Is(err, io.EOF) {
			f.Close()
			return nil, nil, fmt.Errorf("cannot read %s: %w", path, err)
		}
		delim = DetectDelimiter(line)
		if _, err := f.Seek(0, io.SeekStart); err != nil {
			f.Close()
			return nil, nil, err
		}
	}
	r := csv.NewReader(bufio.NewReaderSize(f, 1<<20))
	r.Comma = delim
	r.ReuseRecord = true
	r.FieldsPerRecord = -1 // ragged rows are padded rather than rejected
	r.LazyQuotes = true
	return f, r, nil
}

// DetectDelimiter guesses the delimiter from the header line, defaulting to a comma.
func DetectDelimiter(headerLine string) rune {
	best, bestCount := ',', -1
	for _, c := range []rune{',', ';', '\t', '|'} {
		if n := strings.Count(headerLine, string(c)); n > bestCount {
			best, bestCount = c, n
		}
	}
	return best
}

func readHeader(path string, opt Options) ([]string, error) {
	f, r, err := newReader(path, opt)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	rec, err := r.Read()
	if err != nil {
		return nil, fmt.Errorf("file has no header row: %s", path)
	}
	return slices.Clone(rec), nil
}

// readStore reads a file straight into a RowStore, projecting and normalising as it goes.
func readStore(path string, header, compared []string, opt Options) (*RowStore, error) {
	f, r, err := newReader(path, opt)
	if err != nil {
		return nil, err
	}
	defer f.Close()

	keySize := len(opt.Key)
	width := keySize + len(compared)
	index := make([]int, width)
	for i, name := range opt.Key {
		index[i] = slices.Index(header, name)
	}
	for i, name := range compared {
		index[keySize+i] = slices.Index(header, name)
	}

	store := NewRowStore(opt, compared)
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
		store.Add(row)
	}
	return store, nil
}
