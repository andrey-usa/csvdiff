package csvdiff

import (
	"bufio"
	"encoding/csv"
	"fmt"
	"os"
	"path/filepath"
)

// assemble turns a completed join into the capped report sections, and writes
// the uncapped CSV exports when --export-dir is set.
func assemble(meta EngineMeta, joined Joined, dupA, dupB Section, opt Options, compared []string) (EngineResult, error) {
	cols := append(append([]string{}, opt.Key...), compared...)

	if opt.ExportDir != "" {
		if err := export(joined, opt, compared, cols); err != nil {
			return EngineResult{}, err
		}
	}

	return EngineResult{
		Meta:    meta,
		Counts:  joined.Counts,
		Columns: joined.Columns,
		Changed: CapSection(opt.Key, joined.Changed, opt.MaxRows),
		Added:   CapSection(cols, joined.Added, opt.MaxRows),
		Removed: CapSection(cols, joined.Removed, opt.MaxRows),
		DupA:    dupA,
		DupB:    dupB,
	}, nil
}

func export(joined Joined, opt Options, compared, cols []string) error {
	dir := opt.ExportDir
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("cannot create export-dir %s: %w", dir, err)
	}
	if err := writeCSV(filepath.Join(dir, "added.csv"), cols, joined.Added); err != nil {
		return err
	}
	if err := writeCSV(filepath.Join(dir, "removed.csv"), cols, joined.Removed); err != nil {
		return err
	}

	both := append([]string{}, opt.Key...)
	for _, c := range compared {
		both = append(both, c+" (A)", c+" (B)")
	}
	keySize := len(opt.Key)
	rows := make([][]any, 0, len(joined.ChangedA))
	for i := range joined.ChangedA {
		ar, br := joined.ChangedA[i], joined.ChangedB[i]
		row := make([]any, 0, len(both))
		for j := range keySize {
			row = append(row, ar[j])
		}
		for j := range compared {
			row = append(row, ar[keySize+j], br[keySize+j])
		}
		rows = append(rows, row)
	}
	return writeCSV(filepath.Join(dir, "changed.csv"), both, rows)
}

func writeCSV(path string, header []string, rows [][]any) error {
	f, err := os.Create(path)
	if err != nil {
		return fmt.Errorf("cannot write %s: %w", path, err)
	}
	defer f.Close()

	bw := bufio.NewWriterSize(f, 1<<20)
	w := csv.NewWriter(bw)
	if err := w.Write(header); err != nil {
		return err
	}
	rec := make([]string, len(header))
	for _, row := range rows {
		for i := range rec {
			rec[i] = ""
			if i < len(row) {
				if v, ok := row[i].(*string); ok && v != nil {
					rec[i] = *v
				}
			}
		}
		if err := w.Write(rec); err != nil {
			return err
		}
	}
	w.Flush()
	if err := w.Error(); err != nil {
		return err
	}
	return bw.Flush()
}
