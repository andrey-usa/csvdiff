package csvdiff

import (
	"encoding/json"
	"path/filepath"
	"testing"
)

// The engine-wide tests already run sortmerge alongside every other backend,
// because they enumerate ConcreteEngines. What they never reach is the half of
// this engine that only runs on a file too big for one batch: sorted runs
// written to disk and merged back. Shrinking the batch budget forces it.
func withTinyBatches(t *testing.T) {
	t.Helper()
	original := batchBytes
	batchBytes = 2048
	t.Cleanup(func() { batchBytes = original })
}

func sortMergeOpts(key string) Options {
	o := opts(key)
	o.EngineName = string(EngineSortMerge)
	return o
}

func payloadOf(t *testing.T, res Result) string {
	t.Helper()
	b, err := json.Marshal(map[string]any{
		"counts": res.Counts, "columns": res.Columns, "changed": res.Changed,
		"added": res.Added, "removed": res.Removed, "dup_a": res.DupA, "dup_b": res.DupB,
	})
	if err != nil {
		t.Fatal(err)
	}
	return string(b)
}

func TestSortMergeSpilledRunsMatchTheInMemorySort(t *testing.T) {
	a, b := pair(t, simpleA, simpleB)

	inMemory, err := Compare(a, b, sortMergeOpts("id"))
	if err != nil {
		t.Fatal(err)
	}
	withTinyBatches(t)
	spilled, err := Compare(a, b, sortMergeOpts("id"))
	if err != nil {
		t.Fatal(err)
	}
	if payloadOf(t, spilled) != payloadOf(t, inMemory) {
		t.Errorf("spilling changed the answer:\n got %s\nwant %s",
			payloadOf(t, spilled), payloadOf(t, inMemory))
	}
}

// The row the answer depends on is neither the smallest nor the largest by
// value, so a sort that is not stable, or a merge that does not break ties by
// run, picks one of the other two.
func TestSortMergeKeepsFirstOccurrenceAcrossASpill(t *testing.T) {
	a, b := pair(t,
		"k,v\n3,c\n1,first\n2,b\n1,second\n1,third\n",
		"k,v\n1,first\n2,b\n3,c\n")

	withTinyBatches(t)
	res, err := Compare(a, b, sortMergeOpts("k"))
	if err != nil {
		t.Fatal(err)
	}
	if res.Counts.Changed != 0 {
		t.Errorf("changed = %d, want 0: the first row for key 1 is unchanged", res.Counts.Changed)
	}
	if res.Counts.ADupKeys != 1 || res.Counts.ADupRows != 3 {
		t.Errorf("dup keys/rows = %d/%d, want 1/3", res.Counts.ADupKeys, res.Counts.ADupRows)
	}
}

// Quotes, commas, newlines and absent fields all have to survive the round trip
// through the spill format, not just the CSV reader.
func TestSortMergeSpillFormatRoundTripsAwkwardFields(t *testing.T) {
	a, b := pair(t,
		"k,v,w\n1,\"a\"\"b\",\n2,\"has,comma\",x\n3,\"two\nlines\",y\n",
		"k,v,w\n1,\"a\"\"b\",z\n2,\"has,comma\",x\n3,\"two\nlines\",y\n")

	reference, err := Compare(a, b, opts("k"))
	if err != nil {
		t.Fatal(err)
	}
	withTinyBatches(t)
	spilled, err := Compare(a, b, sortMergeOpts("k"))
	if err != nil {
		t.Fatal(err)
	}
	if spilled.Counts != reference.Counts {
		t.Errorf("counts = %+v, want %+v", spilled.Counts, reference.Counts)
	}
	if reference.Counts.Changed == 0 {
		t.Fatal("the fixture is meant to find a difference")
	}
}

// The fixture in tests/fixtures holds every shape that has broken an engine in
// this project, or plausibly could; its README says which row is which.
func TestSortMergeAgreesOnTheAwkwardFixture(t *testing.T) {
	a := filepath.Join("..", "..", "..", "tests", "fixtures", "awkward_a.csv")
	b := filepath.Join("..", "..", "..", "tests", "fixtures", "awkward_b.csv")

	for _, tc := range []struct {
		name             string
		trim, ignoreCase bool
	}{
		{"defaults", false, false},
		{"trim", true, false},
		{"ignore-case", false, true},
		{"trim+ignore-case", true, true},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var reference string
			for _, engine := range ConcreteEngines {
				if engine == EngineDuckDB && !duckDBAvailable() {
					continue
				}
				o := opts("k")
				o.EngineName = string(engine)
				o.Trim, o.IgnoreCase = tc.trim, tc.ignoreCase
				res, err := Compare(a, b, o)
				if err != nil {
					t.Fatalf("%s: %v", engine, err)
				}
				got := payloadOf(t, res)
				if reference == "" {
					reference = got
					continue
				}
				if got != reference {
					t.Errorf("%s diverges:\n got %s\nwant %s", engine, got, reference)
				}
			}
		})
	}
}
