package main

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func csvPair(t *testing.T, a, b string) (string, string) {
	t.Helper()
	dir := t.TempDir()
	pa, pb := filepath.Join(dir, "a.csv"), filepath.Join(dir, "b.csv")
	if err := os.WriteFile(pa, []byte(a), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(pb, []byte(b), 0o644); err != nil {
		t.Fatal(err)
	}
	return pa, pb
}

func invoke(t *testing.T, args ...string) (int, string, string) {
	t.Helper()
	var out, errOut bytes.Buffer
	status := run(args, &out, &errOut)
	return status, out.String(), errOut.String()
}

const (
	sameA = "id,v\n1,x\n"
	diffB = "id,v\n1,y\n"
)

// The exit codes are the contract a CI pipeline gates on, so they are tested
// rather than assumed.
func TestExitCodeZeroWhenIdentical(t *testing.T) {
	a, b := csvPair(t, sameA, sameA)
	out := filepath.Join(t.TempDir(), "r.html")
	if status, _, errOut := invoke(t, "compare", a, b, "-k", "id", "-o", out); status != 0 {
		t.Errorf("status = %d, want 0 for identical files: %s", status, errOut)
	}
}

func TestExitCodeOneWhenDifferent(t *testing.T) {
	a, b := csvPair(t, sameA, diffB)
	out := filepath.Join(t.TempDir(), "r.html")
	if status, _, errOut := invoke(t, "compare", a, b, "-k", "id", "-o", out); status != 1 {
		t.Errorf("status = %d, want 1 when files differ: %s", status, errOut)
	}
}

func TestExitCodeTwoOnError(t *testing.T) {
	a, b := csvPair(t, sameA, diffB)
	status, _, errOut := invoke(t, "compare", a, b) // no --key
	if status != 2 {
		t.Errorf("status = %d, want 2 without a key", status)
	}
	if !strings.Contains(errOut, "key") {
		t.Errorf("the error should name the missing key, got %q", errOut)
	}
}

func TestExitCodeThreeOnDuplicatesWhenAsked(t *testing.T) {
	a, b := csvPair(t, "id,v\n1,x\n1,x\n", "id,v\n1,x\n")
	out := filepath.Join(t.TempDir(), "r.html")
	status, _, _ := invoke(t, "compare", a, b, "-k", "id", "-o", out, "--fail-on-dups")
	if status != 3 {
		t.Errorf("status = %d, want 3 with --fail-on-dups and a duplicate key", status)
	}
	// Without the flag the same files are merely "identical".
	status, _, _ = invoke(t, "compare", a, b, "-k", "id", "-o", out)
	if status != 0 {
		t.Errorf("status = %d, want 0 without --fail-on-dups", status)
	}
}

func TestUnknownOptionIsRejected(t *testing.T) {
	a, b := csvPair(t, sameA, diffB)
	if status, _, _ := invoke(t, "compare", a, b, "-k", "id", "--nope"); status != 2 {
		t.Error("an unknown option should be an error, not silently ignored")
	}
}

// Everybody types the files last; the parser has to cope.
func TestFilesMayFollowTheOptions(t *testing.T) {
	a, b := csvPair(t, sameA, diffB)
	out := filepath.Join(t.TempDir(), "r.html")
	if status, _, errOut := invoke(t, "compare", "-k", "id", "-o", out, a, b); status != 1 {
		t.Errorf("status = %d, want 1: %s", status, errOut)
	}
	if status, _, errOut := invoke(t, "compare", "--key=id", a, "-o", out, b); status != 1 {
		t.Errorf("--key=id form: status = %d, want 1: %s", status, errOut)
	}
}

func TestJsonSummaryHasCountsButNoRows(t *testing.T) {
	a, b := csvPair(t, sameA, diffB)
	dir := t.TempDir()
	summary := filepath.Join(dir, "s.json")
	invoke(t, "compare", a, b, "-k", "id", "-o", filepath.Join(dir, "r.html"), "--json", summary)

	data, err := os.ReadFile(summary)
	if err != nil {
		t.Fatal(err)
	}
	var parsed map[string]any
	if err := json.Unmarshal(data, &parsed); err != nil {
		t.Fatal(err)
	}
	for _, key := range []string{"meta", "counts", "columns"} {
		if _, ok := parsed[key]; !ok {
			t.Errorf("the summary is missing %q", key)
		}
	}
	for _, key := range []string{"changed", "added", "removed"} {
		if _, ok := parsed[key]; ok {
			t.Errorf("the summary should not embed the %q rows", key)
		}
	}
}

func TestDefaultReportName(t *testing.T) {
	a, b := csvPair(t, sameA, diffB)
	dir := t.TempDir()
	cwd, _ := os.Getwd()
	t.Chdir(dir)
	defer os.Chdir(cwd)

	invoke(t, "compare", a, b, "-k", "id")
	if _, err := os.Stat(filepath.Join(dir, "a__vs__b.html")); err != nil {
		t.Errorf("want a__vs__b.html in the working directory: %v", err)
	}
}

func TestHelpAndUnknownCommand(t *testing.T) {
	if status, out, _ := invoke(t, "--help"); status != 0 || !strings.Contains(out, "usage:") {
		t.Errorf("--help: status %d, out %q", status, out)
	}
	if status, _, errOut := invoke(t, "frobnicate"); status != 2 || !strings.Contains(errOut, "unknown command") {
		t.Errorf("unknown command: status %d, err %q", status, errOut)
	}
	if status, _, _ := invoke(t); status != 2 {
		t.Error("no arguments should be an error with the usage text")
	}
}

func TestComma(t *testing.T) {
	for in, want := range map[int64]string{0: "0", 999: "999", 1000: "1,000",
		1234567: "1,234,567", -1234: "-1,234"} {
		if got := comma(in); got != want {
			t.Errorf("comma(%d) = %s, want %s", in, got, want)
		}
	}
}
