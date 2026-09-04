package csvdiff

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

// Profile is one named comparison from csvdiff.toml, so a recurring comparison
// does not get retyped. Fields are pointers because "absent" and "false"/"0"
// mean different things: only the keys actually present override the defaults.
//
//	[profiles.orders]
//	key       = ["order_id", "line_no"]
//	compare   = ["qty", "price", "status"]   # omit for all common non-key columns
//	ignore    = ["updated_at"]
//	trim      = true
//	tolerance = 0.005
type Profile struct {
	Key         []string
	Compare     []string
	Ignore      []string
	Trim        *bool
	IgnoreCase  *bool
	EmptyIsNull *bool
	Tolerance   *float64
	MaxRows     *int
	Delimiter   *string
	Encoding    *string
	Engine      *string
}

// SearchPath is where a config file is looked for when no path is given.
func SearchPath() []string {
	paths := []string{"csvdiff.toml"}
	if home, err := os.UserHomeDir(); err == nil {
		paths = append(paths, filepath.Join(home, ".config", "csvdiff", "csvdiff.toml"))
	}
	return paths
}

// LoadProfiles reads the profile tables from a config file, or returns an empty
// map when there is none.
//
// This reads the subset of TOML the config format actually uses — [profiles.x]
// tables of scalars and string arrays — rather than pulling in a parser for a
// file that is a dozen lines long.
func LoadProfiles(explicit string) (map[string]Profile, error) {
	candidates := SearchPath()
	if explicit != "" {
		candidates = []string{explicit}
	}
	for _, p := range candidates {
		st, err := os.Stat(p)
		if err != nil || st.IsDir() {
			continue
		}
		f, err := os.Open(p)
		if err != nil {
			return nil, fmt.Errorf("cannot read %s: %w", p, err)
		}
		defer f.Close()
		profiles, err := parseProfiles(f)
		if err != nil {
			return nil, fmt.Errorf("cannot parse %s: %w", p, err)
		}
		return profiles, nil
	}
	if explicit != "" {
		return nil, fmt.Errorf("config file not found: %s", explicit)
	}
	return map[string]Profile{}, nil
}

func parseProfiles(r interface{ Read([]byte) (int, error) }) (map[string]Profile, error) {
	out := map[string]Profile{}
	current := ""
	sc := bufio.NewScanner(r)
	sc.Buffer(make([]byte, 0, 64*1024), 1<<20)
	for sc.Scan() {
		line := strings.TrimSpace(sc.Text())
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		if strings.HasPrefix(line, "[") && strings.HasSuffix(line, "]") {
			name := strings.TrimSpace(line[1 : len(line)-1])
			current = ""
			if after, ok := strings.CutPrefix(name, "profiles."); ok {
				current = strings.Trim(after, `"`)
				if _, seen := out[current]; !seen {
					out[current] = Profile{}
				}
			}
			continue
		}
		if current == "" {
			continue
		}
		key, value, ok := strings.Cut(line, "=")
		if !ok {
			continue
		}
		p := out[current]
		if err := assign(&p, strings.TrimSpace(key), strings.TrimSpace(stripComment(value))); err != nil {
			return nil, err
		}
		out[current] = p
	}
	return out, sc.Err()
}

// stripComment removes a trailing # comment, which cannot appear inside a value
// here because every value is a scalar or a string array.
func stripComment(v string) string {
	inString := false
	for i, c := range v {
		switch c {
		case '"':
			inString = !inString
		case '#':
			if !inString {
				return v[:i]
			}
		}
	}
	return v
}

func assign(p *Profile, key, raw string) error {
	switch key {
	case "key":
		p.Key = tomlArray(raw)
	case "compare":
		p.Compare = tomlArray(raw)
	case "ignore":
		p.Ignore = tomlArray(raw)
	case "trim":
		p.Trim = ptr(raw == "true")
	case "ignore_case":
		p.IgnoreCase = ptr(raw == "true")
	case "empty_is_null":
		p.EmptyIsNull = ptr(raw == "true")
	case "tolerance":
		v, err := strconv.ParseFloat(raw, 64)
		if err != nil {
			return fmt.Errorf("tolerance must be a number, got: %s", raw)
		}
		p.Tolerance = &v
	case "max_rows":
		v, err := strconv.Atoi(raw)
		if err != nil {
			return fmt.Errorf("max_rows must be an integer, got: %s", raw)
		}
		p.MaxRows = &v
	case "delimiter":
		p.Delimiter = ptr(tomlString(raw))
	case "encoding":
		p.Encoding = ptr(tomlString(raw))
	case "engine":
		p.Engine = ptr(tomlString(raw))
	}
	return nil
}

func ptr[T any](v T) *T { return &v }

func tomlString(raw string) string {
	return strings.Trim(strings.TrimSpace(raw), `"'`)
}

func tomlArray(raw string) []string {
	raw = strings.TrimSpace(raw)
	raw = strings.TrimPrefix(raw, "[")
	raw = strings.TrimSuffix(raw, "]")
	var out []string
	for _, part := range strings.Split(raw, ",") {
		if v := tomlString(part); v != "" {
			out = append(out, v)
		}
	}
	return out
}

// Apply lays a profile down under the command-line options, which then override it.
func (o *Options) Apply(p Profile) {
	if p.Key != nil {
		o.Key = p.Key
	}
	if p.Compare != nil {
		o.Compare = p.Compare
	}
	if p.Ignore != nil {
		o.Ignore = p.Ignore
	}
	if p.Trim != nil {
		o.Trim = *p.Trim
	}
	if p.IgnoreCase != nil {
		o.IgnoreCase = *p.IgnoreCase
	}
	if p.EmptyIsNull != nil {
		o.EmptyIsNull = *p.EmptyIsNull
	}
	if p.Tolerance != nil {
		o.Tolerance = *p.Tolerance
	}
	if p.MaxRows != nil {
		o.MaxRows = *p.MaxRows
	}
	if p.Delimiter != nil {
		o.Delimiter = *p.Delimiter
	}
	if p.Encoding != nil {
		o.Encoding = *p.Encoding
	}
	if p.Engine != nil {
		o.EngineName = *p.Engine
	}
}
