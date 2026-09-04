// Package csvdiff compares two CSV files on a composite key and renders a
// self-contained HTML report.
//
// The result contract below is the API, and it is shared byte for byte with the
// Python, TypeScript and Java implementations of this tool: the same JSON, the
// same report template, so a report from any of them is interchangeable. Add a
// field rather than reshaping an existing one.
//
// A CSV cell is a *string that may be nil: values are read as text with no type
// inference, and an empty field — quoted or not — is absent rather than a
// zero-length string.
package csvdiff

import "encoding/json"

// Val is one CSV cell. A nil Val is an absent value.
type Val *string

// Counts are the row totals. Always exact, even when the embedded row lists are
// capped by MaxRows.
type Counts struct {
	ARows     int64 `json:"a_rows"`
	BRows     int64 `json:"b_rows"`
	AKeys     int64 `json:"a_keys"`
	BKeys     int64 `json:"b_keys"`
	Matched   int64 `json:"matched"`
	Unchanged int64 `json:"unchanged"`
	Changed   int64 `json:"changed"`
	Added     int64 `json:"added"`
	Removed   int64 `json:"removed"`
	ADupKeys  int64 `json:"a_dup_keys"`
	ADupRows  int64 `json:"a_dup_rows"`
	BDupKeys  int64 `json:"b_dup_keys"`
	BDupRows  int64 `json:"b_dup_rows"`
}

// ForStatus returns the count for a row status, where the status is only known
// at runtime.
func (c Counts) ForStatus(status string) int64 {
	switch status {
	case "added":
		return c.Added
	case "removed":
		return c.Removed
	case "changed":
		return c.Changed
	case "matched":
		return c.Matched
	default:
		return 0
	}
}

// ColumnStat is how one compared column fared across the matched rows.
type ColumnStat struct {
	Name    string `json:"name"`
	Changed int64  `json:"changed"`
	Blanked int64  `json:"blanked"`
	Filled  int64  `json:"filled"`
}

// CellDiff is one differing cell of a changed row. It serialises as the array
// [columnIndex, a, b]; changed rows carry these triples rather than whole rows,
// and keeping the payload sparse is what keeps a large report small.
type CellDiff struct {
	Column int
	A      *string
	B      *string
}

// MarshalJSON writes the triple as a JSON array, matching the other implementations.
func (c CellDiff) MarshalJSON() ([]byte, error) {
	return json.Marshal([]any{c.Column, c.A, c.B})
}

// Section is a capped list of rows plus the header for them.
//
// For added, removed and duplicate sections a row is a list of cell values; for
// changed rows it is the key values followed by a []CellDiff.
type Section struct {
	Cols      []string `json:"cols"`
	Rows      [][]any  `json:"rows"`
	Truncated bool     `json:"truncated"`
}

// FileMeta describes one of the two compared files.
type FileMeta struct {
	Name string `json:"name"`
	Path string `json:"path"`
	Size int64  `json:"size"`
}

// EngineMeta is what the engine resolved before comparing.
type EngineMeta struct {
	Key      []string `json:"key"`
	Compared []string `json:"compared"`
	OnlyInA  []string `json:"only_in_a"`
	OnlyInB  []string `json:"only_in_b"`
	ACols    int      `json:"a_cols"`
	BCols    int      `json:"b_cols"`
}

// Meta is EngineMeta completed with the run's own facts.
type Meta struct {
	EngineMeta
	A         FileMeta `json:"a"`
	B         FileMeta `json:"b"`
	Engine    string   `json:"engine"`
	Seconds   float64  `json:"seconds"`
	Generated string   `json:"generated"`
	Options   Options  `json:"options"`
}

// EngineResult is what an engine produces, before the run's own facts are attached.
type EngineResult struct {
	Meta    EngineMeta
	Counts  Counts
	Columns []ColumnStat
	Changed Section
	Added   Section
	Removed Section
	DupA    Section
	DupB    Section
}

// Result is a finished comparison.
type Result struct {
	Meta    Meta         `json:"meta"`
	Counts  Counts       `json:"counts"`
	Columns []ColumnStat `json:"columns"`
	Changed Section      `json:"changed"`
	Added   Section      `json:"added"`
	Removed Section      `json:"removed"`
	DupA    Section      `json:"dup_a"`
	DupB    Section      `json:"dup_b"`
}

// Identical reports whether the two files carry the same data under the options given.
func (r Result) Identical() bool {
	return r.Counts.Changed == 0 && r.Counts.Added == 0 && r.Counts.Removed == 0
}

// Summary is the subset written by --json: everything except the embedded rows.
func (r Result) Summary() map[string]any {
	return map[string]any{"meta": r.Meta, "counts": r.Counts, "columns": r.Columns}
}
