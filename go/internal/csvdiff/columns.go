package csvdiff

import (
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"
)

// Resolved is which columns take part, and which exist on only one side.
type Resolved struct {
	Compared []string
	OnlyInA  []string
	OnlyInB  []string
}

// Meta turns the resolution into the contract's EngineMeta.
func (r Resolved) Meta(key []string, aCols, bCols int) EngineMeta {
	return EngineMeta{
		Key:      key,
		Compared: r.Compared,
		OnlyInA:  r.OnlyInA,
		OnlyInB:  r.OnlyInB,
		ACols:    aCols,
		BCols:    bCols,
	}
}

// ResolveColumns works out the compared columns from the two headers.
//
// These rules are what make the engines interchangeable, so they live in one
// place rather than in each backend.
func ResolveColumns(aCols, bCols []string, opt Options) (Resolved, error) {
	var missing []string
	for _, k := range opt.Key {
		if !slices.Contains(aCols, k) || !slices.Contains(bCols, k) {
			missing = append(missing, k)
		}
	}
	if len(missing) > 0 {
		return Resolved{}, fmt.Errorf("key column(s) missing from one of the files: %s", strings.Join(missing, ", "))
	}

	var common []string
	for _, c := range aCols {
		if slices.Contains(bCols, c) {
			common = append(common, c)
		}
	}

	var compared []string
	if opt.Compare == nil {
		for _, c := range common {
			if !slices.Contains(opt.Key, c) {
				compared = append(compared, c)
			}
		}
	} else {
		var bad []string
		for _, c := range opt.Compare {
			if !slices.Contains(common, c) {
				bad = append(bad, c)
			}
		}
		if len(bad) > 0 {
			return Resolved{}, fmt.Errorf("compare column(s) not present in both files: %s", strings.Join(bad, ", "))
		}
		for _, c := range opt.Compare {
			if !slices.Contains(opt.Key, c) {
				compared = append(compared, c)
			}
		}
	}
	compared = slices.DeleteFunc(compared, func(c string) bool { return slices.Contains(opt.Ignore, c) })

	var onlyA, onlyB []string
	for _, c := range aCols {
		if !slices.Contains(bCols, c) {
			onlyA = append(onlyA, c)
		}
	}
	for _, c := range bCols {
		if !slices.Contains(aCols, c) {
			onlyB = append(onlyB, c)
		}
	}
	if compared == nil {
		compared = []string{}
	}
	if onlyA == nil {
		onlyA = []string{}
	}
	if onlyB == nil {
		onlyB = []string{}
	}
	return Resolved{Compared: compared, OnlyInA: onlyA, OnlyInB: onlyB}, nil
}

// Normalise applies --trim, --ignore-case and --empty-is-null to one cell.
func Normalise(v *string, opt Options) *string {
	if v == nil {
		return nil
	}
	s := *v
	if opt.Trim {
		s = strings.TrimSpace(s)
	}
	if opt.IgnoreCase {
		s = strings.ToLower(s)
	}
	if opt.EmptyIsNull && s == "" {
		return nil
	}
	return &s
}

var numberRe = regexp.MustCompile(`^[+-]?(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$`)

func asNumber(v *string) (float64, bool) {
	if v == nil {
		return 0, false
	}
	s := strings.TrimSpace(*v)
	if !numberRe.MatchString(s) {
		return 0, false
	}
	f, err := strconv.ParseFloat(s, 64)
	return f, err == nil
}

// Differs is SQL's IS DISTINCT FROM, with the numeric tolerance applied where
// both sides parse as numbers. Two absent values are equal; one absent value
// differs from any present one.
func Differs(a, b *string, opt Options) bool {
	if a == nil && b == nil {
		return false
	}
	if opt.Tolerance > 0 {
		na, aok := asNumber(a)
		nb, bok := asNumber(b)
		if aok && bok {
			d := na - nb
			if d < 0 {
				d = -d
			}
			return d > opt.Tolerance
		}
	}
	if a == nil || b == nil {
		return true
	}
	return *a != *b
}

// CompareKeys orders key values the way DuckDB orders VARCHAR: ascending, with
// absent values last.
func CompareKeys(x, y []*string, keySize int) int {
	for i := range keySize {
		a, b := x[i], y[i]
		if a == nil && b == nil {
			continue
		}
		if a == nil {
			return 1
		}
		if b == nil {
			return -1
		}
		if c := strings.Compare(*a, *b); c != 0 {
			return c
		}
	}
	return 0
}

// KeyOf flattens a composite key into one string for hashing. \x00 marks an
// absent value and \x01 separates columns; neither can appear in a CSV field,
// so distinct keys cannot collide.
func KeyOf(row []*string, keySize int) string {
	var sb strings.Builder
	for i := range keySize {
		if row[i] == nil {
			sb.WriteByte(0)
		} else {
			sb.WriteString(*row[i])
		}
		sb.WriteByte(1)
	}
	return sb.String()
}

// EmptyToNull treats an empty field as absent, quoted or not.
//
// This is what DuckDB's reader does, and the other implementations follow it,
// so the Go engines must too or a count would change with the engine.
func EmptyToNull(s string) *string {
	if s == "" {
		return nil
	}
	return &s
}
