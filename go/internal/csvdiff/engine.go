package csvdiff

import (
	"fmt"
	"os"
	"path/filepath"
	"time"
)

// engineFunc is the one thing a backend has to provide.
type engineFunc func(aPath, bPath string, opt Options) (EngineResult, error)

// engines maps a name to its backend. Every entry must return identical Counts
// and Columns for the same input; the parity test asserts it.
var engines = map[Engine]engineFunc{
	EngineDuckDB:    compareDuckDB,
	EngineSortMerge: compareSortMerge,
	EngineNative:    compareNative,
}

// Compare runs the comparison and attaches the run's own facts to the result.
func Compare(aPath, bPath string, opt Options) (Result, error) {
	if err := opt.Validate(); err != nil {
		return Result{}, err
	}
	for _, p := range []string{aPath, bPath} {
		if st, err := os.Stat(p); err != nil || st.IsDir() {
			return Result{}, fmt.Errorf("file not found: %s", p)
		}
	}

	engine, err := ResolveEngine(Engine(opt.EngineName), aPath, bPath)
	if err != nil {
		return Result{}, err
	}

	start := time.Now()
	res, err := engines[engine](aPath, bPath, opt)
	if err != nil {
		return Result{}, fmt.Errorf("the %s engine failed: %w", engine, err)
	}
	seconds := float64(int64(time.Since(start).Round(time.Millisecond))) / float64(time.Second)

	meta := Meta{
		EngineMeta: res.Meta,
		A:          fileMeta(aPath),
		B:          fileMeta(bPath),
		Engine:     string(engine),
		Seconds:    seconds,
		Generated:  time.Now().Truncate(time.Second).Format(time.RFC3339),
		Options:    opt,
	}
	return Result{
		Meta: meta, Counts: res.Counts, Columns: res.Columns,
		Changed: res.Changed, Added: res.Added, Removed: res.Removed,
		DupA: res.DupA, DupB: res.DupB,
	}, nil
}

// ResolveEngine turns EngineAuto into a concrete backend.
//
// DuckDB is preferred because it streams from disk, but it is only worth its
// start-up cost — and its cgo dependency — when it can actually be loaded, so
// auto falls through to native if it cannot.
func ResolveEngine(requested Engine, aPath, bPath string) (Engine, error) {
	if requested != EngineAuto {
		if _, ok := engines[requested]; !ok {
			return "", fmt.Errorf("unknown engine: %s", requested)
		}
		return requested, nil
	}
	for _, candidate := range ConcreteEngines {
		if available(candidate) {
			return candidate, nil
		}
	}
	return EngineNative, nil
}

// available reports whether a backend can run in this build.
func available(e Engine) bool {
	switch e {
	case EngineDuckDB:
		return duckDBAvailable()
	case EngineSortMerge, EngineNative:
		return true
	default:
		return false
	}
}

func fileMeta(p string) FileMeta {
	abs, err := filepath.Abs(p)
	if err != nil {
		abs = p
	}
	var size int64
	if st, err := os.Stat(p); err == nil {
		size = st.Size()
	}
	return FileMeta{Name: filepath.Base(p), Path: abs, Size: size}
}
