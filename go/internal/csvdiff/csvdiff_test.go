package csvdiff

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// write puts a CSV in the test's temp directory and returns its path.
func write(t *testing.T, name, body string) string {
	t.Helper()
	p := filepath.Join(t.TempDir(), name)
	if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
		t.Fatal(err)
	}
	return p
}

func pair(t *testing.T, a, b string) (string, string) {
	t.Helper()
	dir := t.TempDir()
	pa, pb := filepath.Join(dir, "a.csv"), filepath.Join(dir, "b.csv")
	for p, body := range map[string]string{pa: a, pb: b} {
		if err := os.WriteFile(p, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return pa, pb
}

func opts(key ...string) Options {
	o := NewOptions()
	o.Key = key
	return o
}

// eachEngine runs a check against every concrete backend, because the whole
// point of the engine registry is that they are interchangeable.
func eachEngine(t *testing.T, run func(t *testing.T, engine Engine)) {
	t.Helper()
	for _, e := range ConcreteEngines {
		if !available(e) {
			t.Logf("skipping %s: not available here", e)
			continue
		}
		t.Run(string(e), func(t *testing.T) { run(t, e) })
	}
}

const (
	simpleA = "id,name,qty\n1,ann,5\n2,bob,7\n3,cid,9\n"
	simpleB = "id,name,qty\n1,ann,5\n2,bob,8\n4,dee,1\n"
)

func TestCountsAcrossEngines(t *testing.T) {
	a, b := pair(t, simpleA, simpleB)
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("id")
		o.EngineName = string(engine)
		got, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		want := Counts{ARows: 3, BRows: 3, AKeys: 3, BKeys: 3, Matched: 2, Unchanged: 1,
			Changed: 1, Added: 1, Removed: 1}
		if got.Counts != want {
			t.Errorf("counts = %+v, want %+v", got.Counts, want)
		}
		if got.Identical() {
			t.Error("Identical() = true for files that differ")
		}
	})
}

// The engines must agree cell for cell, not just on the totals: a report from
// one is meant to be indistinguishable from a report from another.
func TestEngineParity(t *testing.T) {
	a, b := pair(t, simpleA, simpleB)
	var reference string
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("id")
		o.EngineName = string(engine)
		res, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		payload, err := json.Marshal(map[string]any{
			"counts": res.Counts, "columns": res.Columns, "changed": res.Changed,
			"added": res.Added, "removed": res.Removed, "dup_a": res.DupA, "dup_b": res.DupB,
		})
		if err != nil {
			t.Fatal(err)
		}
		if reference == "" {
			reference = string(payload)
			return
		}
		if string(payload) != reference {
			t.Errorf("%s diverges from the first engine:\n got %s\nwant %s", engine, payload, reference)
		}
	})
}

func TestIdenticalFiles(t *testing.T) {
	a, b := pair(t, simpleA, simpleA)
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("id")
		o.EngineName = string(engine)
		res, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		if !res.Identical() {
			t.Errorf("Identical() = false for a file compared with itself: %+v", res.Counts)
		}
	})
}

// An empty field is an absent value whether or not it was quoted. Every engine
// has to agree, or a count would change with the engine.
func TestQuotedEmptyIsNull(t *testing.T) {
	a, b := pair(t, "k,v\n1,\n2,\"\"\n3,keep\n", "k,v\n1,\"\"\n2,\n3,keep\n")
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("k")
		o.EngineName = string(engine)
		res, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		if res.Counts.Changed != 0 {
			t.Errorf("changed = %d, want 0: an empty field is absent, quoted or not", res.Counts.Changed)
		}
	})
}

func TestDuplicateKeysFirstOccurrenceJoins(t *testing.T) {
	a, b := pair(t, "k,v\n1,first\n1,second\n2,x\n", "k,v\n1,first\n2,y\n")
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("k")
		o.EngineName = string(engine)
		res, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		if res.Counts.ADupKeys != 1 || res.Counts.ADupRows != 2 {
			t.Errorf("dup keys/rows in A = %d/%d, want 1/2", res.Counts.ADupKeys, res.Counts.ADupRows)
		}
		// Only key 2 changed: the first occurrence of key 1 matched.
		if res.Counts.Changed != 1 {
			t.Errorf("changed = %d, want 1 (the first occurrence of a duplicated key joins)", res.Counts.Changed)
		}
		if len(res.DupA.Rows) != 1 {
			t.Errorf("dup_a rows = %d, want 1", len(res.DupA.Rows))
		}
	})
}

func TestToleranceAndNormalisation(t *testing.T) {
	a, b := pair(t, "k,v,s\n1,10.00, Ann \n", "k,v,s\n1,10.004,ann\n")
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("k")
		o.EngineName = string(engine)
		o.Tolerance = 0.01
		o.Trim = true
		o.IgnoreCase = true
		res, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		if res.Counts.Changed != 0 {
			t.Errorf("changed = %d, want 0 under --tolerance/--trim/--ignore-case", res.Counts.Changed)
		}
	})
}

func TestBlankedAndFilledColumnStats(t *testing.T) {
	a, b := pair(t, "k,v\n1,x\n2,\n", "k,v\n1,\n2,y\n")
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("k")
		o.EngineName = string(engine)
		res, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		if len(res.Columns) != 1 {
			t.Fatalf("columns = %d, want 1", len(res.Columns))
		}
		got := res.Columns[0]
		want := ColumnStat{Name: "v", Changed: 2, Blanked: 1, Filled: 1}
		if got != want {
			t.Errorf("column stat = %+v, want %+v", got, want)
		}
	})
}

func TestIgnoreAndCompareSelection(t *testing.T) {
	a, b := pair(t, "k,x,y\n1,a,b\n", "k,x,y\n1,A,B\n")
	o := opts("k")
	o.Ignore = []string{"x"}
	o.EngineName = string(EngineNative)
	res, err := Compare(a, b, o)
	if err != nil {
		t.Fatal(err)
	}
	if len(res.Meta.Compared) != 1 || res.Meta.Compared[0] != "y" {
		t.Errorf("compared = %v, want [y]", res.Meta.Compared)
	}
}

func TestMissingKeyColumnIsAnError(t *testing.T) {
	a, b := pair(t, "k,v\n1,x\n", "j,v\n1,x\n")
	o := opts("k")
	o.EngineName = string(EngineNative)
	if _, err := Compare(a, b, o); err == nil {
		t.Fatal("want an error when the key is missing from one file")
	}
}

func TestMissingFileIsAnError(t *testing.T) {
	a := write(t, "a.csv", simpleA)
	if _, err := Compare(a, filepath.Join(t.TempDir(), "nope.csv"), opts("id")); err == nil {
		t.Fatal("want an error for a missing file")
	}
}

func TestExportDirWritesFullCsvs(t *testing.T) {
	a, b := pair(t, simpleA, simpleB)
	eachEngine(t, func(t *testing.T, engine Engine) {
		dir := filepath.Join(t.TempDir(), "nested", "export")
		o := opts("id")
		o.EngineName = string(engine)
		o.ExportDir = dir
		if _, err := Compare(a, b, o); err != nil {
			t.Fatal(err)
		}
		for _, name := range []string{"changed.csv", "added.csv", "removed.csv"} {
			data, err := os.ReadFile(filepath.Join(dir, name))
			if err != nil {
				t.Fatalf("%s: %v", name, err)
			}
			if len(strings.Split(strings.TrimSpace(string(data)), "\n")) != 2 {
				t.Errorf("%s should hold a header and one row, got %q", name, data)
			}
		}
	})
}

func TestMaxRowsCapsButCountsStayExact(t *testing.T) {
	var a, b strings.Builder
	a.WriteString("k,v\n")
	b.WriteString("k,v\n")
	for i := range 10 {
		a.WriteString(strings.Join([]string{string(rune('a' + i)), "x"}, ",") + "\n")
		b.WriteString(strings.Join([]string{string(rune('a' + i)), "y"}, ",") + "\n")
	}
	pa, pb := pair(t, a.String(), b.String())
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("k")
		o.EngineName = string(engine)
		o.MaxRows = 3
		res, err := Compare(pa, pb, o)
		if err != nil {
			t.Fatal(err)
		}
		if res.Counts.Changed != 10 {
			t.Errorf("changed count = %d, want the exact 10 even when capped", res.Counts.Changed)
		}
		if len(res.Changed.Rows) != 3 || !res.Changed.Truncated {
			t.Errorf("changed section = %d rows truncated=%v, want 3 and true",
				len(res.Changed.Rows), res.Changed.Truncated)
		}
	})
}

func TestSemicolonDelimiterIsSniffed(t *testing.T) {
	a, b := pair(t, "k;v\n1;x\n", "k;v\n1;y\n")
	eachEngine(t, func(t *testing.T, engine Engine) {
		o := opts("k")
		o.EngineName = string(engine)
		res, err := Compare(a, b, o)
		if err != nil {
			t.Fatal(err)
		}
		if res.Counts.Changed != 1 {
			t.Errorf("changed = %d, want 1 with a sniffed semicolon delimiter", res.Counts.Changed)
		}
	})
}

func TestValidateRejectsBadOptions(t *testing.T) {
	for name, mutate := range map[string]func(*Options){
		"no key":             func(o *Options) { o.Key = nil },
		"negative tolerance": func(o *Options) { o.Tolerance = -1 },
		"zero max rows":      func(o *Options) { o.MaxRows = 0 },
		"unknown engine":     func(o *Options) { o.EngineName = "nope" },
		"long delimiter":     func(o *Options) { o.Delimiter = ";;" },
	} {
		t.Run(name, func(t *testing.T) {
			o := opts("k")
			mutate(&o)
			if err := o.Validate(); err == nil {
				t.Errorf("Validate() accepted %s", name)
			}
		})
	}
}

func TestDetectDelimiter(t *testing.T) {
	for line, want := range map[string]rune{
		"a,b,c":   ',',
		"a;b;c":   ';',
		"a\tb\tc": '\t',
		"a|b|c":   '|',
		"single":  ',',
	} {
		if got := DetectDelimiter(line); got != want {
			t.Errorf("DetectDelimiter(%q) = %q, want %q", line, got, want)
		}
	}
}

func TestDiffers(t *testing.T) {
	s := func(v string) *string { return &v }
	o := NewOptions()
	if Differs(nil, nil, o) {
		t.Error("two absent values must be equal")
	}
	if !Differs(nil, s(""), o) {
		t.Error("absent must differ from an empty string when --empty-is-null is off")
	}
	o.EmptyIsNull = true
	if Differs(nil, Normalise(s(""), o), o) {
		t.Error("--empty-is-null should make an empty string absent")
	}
}
