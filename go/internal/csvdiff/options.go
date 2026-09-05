package csvdiff

import (
	"fmt"
	"slices"
	"strings"
)

// Engine names the comparison backend.
//
// EngineAuto resolves to the first concrete engine that can run here, in
// declaration order. Every concrete engine must return identical Counts and
// Columns for the same input; the tests and CI both assert that.
type Engine string

const (
	// EngineAuto picks the first available concrete engine.
	EngineAuto Engine = "auto"
	// EngineDuckDB runs the comparison in DuckDB: out-of-core, handles files larger than RAM.
	EngineDuckDB Engine = "duckdb"
	// EngineSortMerge sorts both files by key and walks them together: bounded
	// memory, spills to disk, size limited by disk rather than RAM.
	EngineSortMerge Engine = "sortmerge"
	// EngineNative parses with encoding/csv and joins in a map. In-memory only.
	EngineNative Engine = "native"
)

// ConcreteEngines lists the real backends in EngineAuto preference order.
var ConcreteEngines = []Engine{EngineDuckDB, EngineSortMerge, EngineNative}

// ParseEngine validates a command-line spelling.
func ParseEngine(s string) (Engine, error) {
	e := Engine(strings.ToLower(strings.TrimSpace(s)))
	if e == EngineAuto || slices.Contains(ConcreteEngines, e) {
		return e, nil
	}
	valid := []string{string(EngineAuto)}
	for _, c := range ConcreteEngines {
		valid = append(valid, string(c))
	}
	return "", fmt.Errorf("unknown engine: %s. Choose one of %s", s, strings.Join(valid, ", "))
}

// DefaultMaxRows is how many rows each report section embeds by default.
// Counts are always exact regardless.
const DefaultMaxRows = 50_000

// Options is everything that varies per comparison. Nothing about a specific
// dataset belongs in the code, so key columns, compared columns and
// normalisation are all runtime parameters.
//
// The JSON names are snake_case to match the other implementations, which read
// and write the same contract.
type Options struct {
	// Key is the composite key; at least one column is required.
	Key []string `json:"-"`
	// Compare lists the columns to diff, or nil for every common non-key column.
	Compare []string `json:"-"`
	// Ignore lists columns to skip entirely.
	Ignore []string `json:"-"`

	Trim        bool    `json:"trim"`
	IgnoreCase  bool    `json:"ignore_case"`
	EmptyIsNull bool    `json:"empty_is_null"`
	Tolerance   float64 `json:"tolerance"`
	MaxRows     int     `json:"max_rows"`
	Delimiter   string  `json:"delimiter"`
	Encoding    string  `json:"encoding"`
	EngineName  string  `json:"engine"`
	Threads     int     `json:"threads"`
	MemoryLimit string  `json:"memory_limit"`
	ExportDir   string  `json:"export_dir"`
}

// NewOptions returns the defaults, which callers then override.
func NewOptions() Options {
	return Options{
		MaxRows:    DefaultMaxRows,
		Encoding:   "utf-8",
		EngineName: string(EngineAuto),
	}
}

// Validate checks the options are usable and normalises the defaults.
func (o *Options) Validate() error {
	if len(o.Key) == 0 {
		return fmt.Errorf("at least one key column is required")
	}
	if o.Tolerance < 0 {
		return fmt.Errorf("--tolerance must not be negative, got %v", o.Tolerance)
	}
	if o.MaxRows <= 0 {
		return fmt.Errorf("--max-rows must be positive, got %d", o.MaxRows)
	}
	if o.Threads < 0 {
		return fmt.Errorf("--threads must not be negative, got %d", o.Threads)
	}
	if o.Encoding == "" {
		o.Encoding = "utf-8"
	}
	if o.EngineName == "" {
		o.EngineName = string(EngineAuto)
	}
	if _, err := ParseEngine(o.EngineName); err != nil {
		return err
	}
	if o.Delimiter != "" && len([]rune(o.Delimiter)) != 1 {
		return fmt.Errorf("--delimiter must be a single character, got %q", o.Delimiter)
	}
	return nil
}

// DelimiterRune returns the forced delimiter, or 0 when it should be sniffed.
func (o Options) DelimiterRune() rune {
	if o.Delimiter == "" {
		return 0
	}
	return []rune(o.Delimiter)[0]
}

// ParseList splits "a, b ,c" into a slice; an empty string yields nil.
func ParseList(s string) []string {
	if strings.TrimSpace(s) == "" {
		return nil
	}
	parts := strings.Split(s, ",")
	out := make([]string, 0, len(parts))
	for _, p := range parts {
		if v := strings.TrimSpace(p); v != "" {
			out = append(out, v)
		}
	}
	return out
}
