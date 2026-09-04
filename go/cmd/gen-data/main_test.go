package main

import (
	"crypto/md5"
	"encoding/hex"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

// The generated files are shared with the Python, TypeScript and Java
// generators byte for byte, so a benchmark number from any of them is directly
// comparable. These digests pin that: if the hash, the drift recipe or the
// number formatting moves, they change and the ports have diverged.
func TestGeneratedFilesMatchTheOtherImplementations(t *testing.T) {
	dir := t.TempDir()
	a := filepath.Join(dir, "a.csv")
	b := filepath.Join(dir, "b.csv")
	if err := generate(20_000, a, b, 7); err != nil {
		t.Fatal(err)
	}
	for path, want := range map[string]string{
		a: "24d0878e98786171b18ce5323fc02ee9",
		b: "016a5cdb3f2d4f24687df5a041c4cbe5",
	} {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		sum := md5.Sum(data)
		if got := hex.EncodeToString(sum[:]); got != want {
			t.Errorf("%s digest = %s, want %s: the generators have diverged",
				filepath.Base(path), got, want)
		}
	}
}

func TestDriftRecipe(t *testing.T) {
	dir := t.TempDir()
	a := filepath.Join(dir, "a.csv")
	b := filepath.Join(dir, "b.csv")
	if err := generate(10_000, a, b, 7); err != nil {
		t.Fatal(err)
	}
	linesOf := func(p string) []string {
		data, err := os.ReadFile(p)
		if err != nil {
			t.Fatal(err)
		}
		return strings.Split(strings.TrimSuffix(string(data), "\n"), "\n")
	}
	aLines, bLines := linesOf(a), linesOf(b)

	if aLines[0] != strings.Join(Columns, ",") {
		t.Errorf("header = %q", aLines[0])
	}
	if len(strings.Split(aLines[1], ",")) != 20 {
		t.Errorf("a row should have 20 fields, got %d", len(strings.Split(aLines[1], ",")))
	}
	// 10k rows, 1 in 1000 removed, 1 duplicate row added to A, 10 added to B, 1 duplicated in B.
	if len(aLines) != 10_000+1+1 {
		t.Errorf("A has %d lines, want %d", len(aLines), 10_002)
	}
	if len(bLines) != 10_000-10+1+10+1 {
		t.Errorf("B has %d lines, want %d", len(bLines), 10_002)
	}
}

func TestParseRows(t *testing.T) {
	for in, want := range map[string]int64{
		"10000": 10_000, "10k": 10_000, "1m": 1_000_000,
		"2.5M": 2_500_000, "1g": 1_000_000_000, "1_000": 1_000,
	} {
		got, err := ParseRows(in)
		if err != nil {
			t.Fatalf("ParseRows(%q): %v", in, err)
		}
		if got != want {
			t.Errorf("ParseRows(%q) = %d, want %d", in, got, want)
		}
	}
	if _, err := ParseRows("many"); err == nil {
		t.Error("ParseRows should reject a non-number")
	}
}

// Go rounds half to even by default; the other implementations round half up,
// and a single differing cent would break the byte-for-byte guarantee.
func TestFixedRoundsHalfUp(t *testing.T) {
	for in, want := range map[float64]string{
		0.125: "0.13", 0.135: "0.14", -0.125: "-0.13", 2.5: "2.50", 1.005: "1.00",
	} {
		var sb strings.Builder
		fixed(&sb, in, 2)
		if got := sb.String(); got != want {
			t.Errorf("fixed(%v) = %s, want %s", in, got, want)
		}
	}
}
