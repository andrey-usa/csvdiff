package csvdiff

import (
	"os"
	"path/filepath"
	"testing"
)

const configTOML = `
# a comment
[profiles.orders]
key       = ["order_id", "line_no"]
compare   = ["qty", "price"]
ignore    = ["updated_at"]   # trailing comment
trim      = true
tolerance = 0.005
max_rows  = 100
engine    = "native"

[profiles.other]
key = ["id"]
`

func TestLoadProfiles(t *testing.T) {
	path := filepath.Join(t.TempDir(), "csvdiff.toml")
	if err := os.WriteFile(path, []byte(configTOML), 0o644); err != nil {
		t.Fatal(err)
	}
	profiles, err := LoadProfiles(path)
	if err != nil {
		t.Fatal(err)
	}
	if len(profiles) != 2 {
		t.Fatalf("profiles = %d, want 2", len(profiles))
	}

	o := NewOptions()
	o.Apply(profiles["orders"])
	if len(o.Key) != 2 || o.Key[1] != "line_no" {
		t.Errorf("key = %v", o.Key)
	}
	if len(o.Ignore) != 1 || o.Ignore[0] != "updated_at" {
		t.Errorf("a trailing comment leaked into ignore: %v", o.Ignore)
	}
	if !o.Trim || o.Tolerance != 0.005 || o.MaxRows != 100 || o.EngineName != "native" {
		t.Errorf("scalars did not apply: %+v", o)
	}
}

// A profile is the base layer; anything given on the command line wins.
func TestCommandLineOverridesProfile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "csvdiff.toml")
	os.WriteFile(path, []byte(configTOML), 0o644)
	profiles, err := LoadProfiles(path)
	if err != nil {
		t.Fatal(err)
	}
	o := NewOptions()
	o.Apply(profiles["orders"])
	o.MaxRows = 7 // as the CLI would, after applying the profile
	if o.MaxRows != 7 {
		t.Errorf("max rows = %d, want the command-line 7", o.MaxRows)
	}
}

func TestMissingConfigPathIsAnError(t *testing.T) {
	if _, err := LoadProfiles(filepath.Join(t.TempDir(), "nope.toml")); err == nil {
		t.Error("want an error when an explicit --config does not exist")
	}
}

func TestParseList(t *testing.T) {
	got := ParseList(" a, b ,,c ")
	want := []string{"a", "b", "c"}
	if len(got) != len(want) {
		t.Fatalf("ParseList = %v, want %v", got, want)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Errorf("ParseList[%d] = %q, want %q", i, got[i], want[i])
		}
	}
	if ParseList("  ") != nil {
		t.Error("an empty string should yield no columns at all, not one empty one")
	}
}
