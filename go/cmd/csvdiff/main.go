// Command csvdiff compares two CSV files on a composite key and writes a
// self-contained HTML report.
//
//	csvdiff compare A.csv B.csv --key id,region [--compare c1,c2] [--out report.html]
//	csvdiff compare A.csv B.csv --profile orders
//
// Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (only
// with --fail-on-dups). That makes it a drop-in CI or pipeline gate.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/andrey-usa/csvdiff/go/internal/csvdiff"
)

const usage = `usage: csvdiff <command> [options]

commands:
  compare A B     Compare two CSV files on a composite key and write an HTML report

compare options:
  -k, --key COLS          Comma-separated key column(s) (or from --profile)
  -c, --compare COLS      Columns to compare (default: all common non-key columns)
  -i, --ignore COLS       Columns to skip
  -p, --profile NAME      Profile name from csvdiff.toml
      --config PATH       Path to csvdiff.toml
      --trim              Strip whitespace before comparing
      --ignore-case
      --empty-is-null     Treat empty string and null as equal
      --tolerance N       Absolute numeric tolerance
      --max-rows N        Rows embedded per section (default 50000)
      --delimiter D       Force delimiter (default: auto)
      --encoding ENC
      --engine E          auto | duckdb | sortmerge | native
      --threads N
      --memory-limit S    DuckDB memory limit, e.g. 4GB
      --export-dir DIR    Write full changed/added/removed CSVs here
  -o, --out PATH          Report path (default: <a>__vs__<b>.html)
      --json PATH         Also write a JSON summary (counts + column stats) here
      --no-compress       Embed plain JSON instead of gzip (older browsers)
      --fail-on-dups      Exit 3 when either file has duplicate keys

Exit codes: 0 identical, 1 differences found, 2 error, 3 duplicate keys (with --fail-on-dups).
`

func main() {
	status := run(os.Args[1:], os.Stdout, os.Stderr)
	reportPeakRSS(os.Stderr)
	os.Exit(status)
}

func run(args []string, out, errOut io.Writer) int {
	if len(args) == 0 {
		fmt.Fprint(errOut, usage)
		return 2
	}
	switch args[0] {
	case "-h", "--help", "help":
		fmt.Fprint(out, usage)
		return 0
	case "compare":
		status, err := compare(args[1:], out)
		if err != nil {
			fmt.Fprintln(errOut, "error:", err)
			return 2
		}
		return status
	default:
		fmt.Fprintf(errOut, "error: unknown command: %s\n\n%s", args[0], usage)
		return 2
	}
}

// flags holds the command line before it is folded into Options, so a flag that
// was never given can be told apart from one given its zero value.
type flagSet struct {
	fs *flag.FlagSet

	key, compare, ignore     string
	profile, config          string
	trim, ignoreCase         bool
	emptyIsNull, noCompress  bool
	failOnDups               bool
	tolerance                float64
	maxRows, threads         int
	delimiter, encoding      string
	engine, memoryLimit      string
	exportDir, out, jsonPath string
	help                     bool
}

func newFlagSet() *flagSet {
	f := &flagSet{fs: flag.NewFlagSet("compare", flag.ContinueOnError)}
	f.fs.SetOutput(io.Discard)
	str := func(p *string, names ...string) {
		for _, n := range names {
			f.fs.StringVar(p, n, "", "")
		}
	}
	bl := func(p *bool, names ...string) {
		for _, n := range names {
			f.fs.BoolVar(p, n, false, "")
		}
	}
	str(&f.key, "key", "k")
	str(&f.compare, "compare", "c")
	str(&f.ignore, "ignore", "i")
	str(&f.profile, "profile", "p")
	str(&f.config, "config")
	str(&f.delimiter, "delimiter")
	str(&f.encoding, "encoding")
	str(&f.engine, "engine")
	str(&f.memoryLimit, "memory-limit")
	str(&f.exportDir, "export-dir")
	str(&f.out, "out", "o")
	str(&f.jsonPath, "json")
	bl(&f.trim, "trim")
	bl(&f.ignoreCase, "ignore-case")
	bl(&f.emptyIsNull, "empty-is-null")
	bl(&f.noCompress, "no-compress")
	bl(&f.failOnDups, "fail-on-dups")
	bl(&f.help, "help", "h")
	f.fs.Float64Var(&f.tolerance, "tolerance", -1, "")
	f.fs.IntVar(&f.maxRows, "max-rows", 0, "")
	f.fs.IntVar(&f.threads, "threads", 0, "")
	return f
}

// parse splits options from file arguments, so the two paths may appear before,
// after or between the flags. Go's flag package stops at the first
// non-flag token, which would reject the ordering everyone actually types.
func (f *flagSet) parse(args []string) ([]string, error) {
	var opts, positional []string
	for i := 0; i < len(args); i++ {
		t := args[i]
		if !strings.HasPrefix(t, "-") || t == "-" {
			positional = append(positional, t)
			continue
		}
		opts = append(opts, t)
		name, _, hasValue := strings.Cut(strings.TrimLeft(t, "-"), "=")
		if hasValue {
			continue
		}
		known := f.fs.Lookup(name)
		if known == nil {
			return nil, fmt.Errorf("unknown option: %s", t)
		}
		if bf, ok := known.Value.(interface{ IsBoolFlag() bool }); ok && bf.IsBoolFlag() {
			continue
		}
		if i+1 >= len(args) {
			return nil, fmt.Errorf("%s needs a value", t)
		}
		i++
		opts = append(opts, args[i])
	}
	if err := f.fs.Parse(opts); err != nil {
		return nil, err
	}
	return positional, nil
}

func compare(args []string, out io.Writer) (int, error) {
	f := newFlagSet()
	positional, err := f.parse(args)
	if err != nil {
		return 0, err
	}
	if f.help {
		fmt.Fprint(out, usage)
		return 0, nil
	}
	if len(positional) < 2 {
		return 0, fmt.Errorf("compare needs two files: csvdiff compare A.csv B.csv --key ...")
	}
	aPath, bPath := positional[0], positional[1]

	opt := csvdiff.NewOptions()
	if f.profile != "" {
		profiles, err := csvdiff.LoadProfiles(f.config)
		if err != nil {
			return 0, err
		}
		p, ok := profiles[f.profile]
		if !ok {
			return 0, fmt.Errorf("profile not found: %s", f.profile)
		}
		opt.Apply(p)
	}

	// Only flags actually present override the profile, hence the guards.
	f.fs.Visit(func(fl *flag.Flag) {
		switch fl.Name {
		case "key", "k":
			opt.Key = csvdiff.ParseList(f.key)
		case "compare", "c":
			opt.Compare = csvdiff.ParseList(f.compare)
		case "ignore", "i":
			opt.Ignore = csvdiff.ParseList(f.ignore)
		case "trim":
			opt.Trim = f.trim
		case "ignore-case":
			opt.IgnoreCase = f.ignoreCase
		case "empty-is-null":
			opt.EmptyIsNull = f.emptyIsNull
		case "tolerance":
			opt.Tolerance = f.tolerance
		case "max-rows":
			opt.MaxRows = f.maxRows
		case "delimiter":
			opt.Delimiter = f.delimiter
		case "encoding":
			opt.Encoding = f.encoding
		case "engine":
			opt.EngineName = f.engine
		case "threads":
			opt.Threads = f.threads
		case "memory-limit":
			opt.MemoryLimit = f.memoryLimit
		case "export-dir":
			opt.ExportDir = f.exportDir
		}
	})
	if len(opt.Key) == 0 {
		return 0, fmt.Errorf("--key (or a profile with key) is required")
	}

	result, err := csvdiff.Compare(aPath, bPath, opt)
	if err != nil {
		return 0, err
	}

	outPath := f.out
	if outPath == "" {
		outPath = stem(aPath) + "__vs__" + stem(bPath) + ".html"
	}
	html, err := csvdiff.Render(result, !f.noCompress)
	if err != nil {
		return 0, err
	}
	if err := os.WriteFile(outPath, []byte(html), 0o644); err != nil {
		return 0, fmt.Errorf("cannot write %s: %w", outPath, err)
	}

	if f.jsonPath != "" {
		summary, err := json.MarshalIndent(result.Summary(), "", "  ")
		if err != nil {
			return 0, err
		}
		if err := os.WriteFile(f.jsonPath, summary, 0o644); err != nil {
			return 0, fmt.Errorf("cannot write %s: %w", f.jsonPath, err)
		}
	}

	c := result.Counts
	fmt.Fprintf(out,
		"A %s rows | B %s rows | matched %s (changed %s) | added %s | removed %s | "+
			"dup keys A %s B %s | %s %gs\n",
		comma(c.ARows), comma(c.BRows), comma(c.Matched), comma(c.Changed),
		comma(c.Added), comma(c.Removed), comma(c.ADupKeys), comma(c.BDupKeys),
		result.Meta.Engine, result.Meta.Seconds)
	if st, err := os.Stat(outPath); err == nil {
		fmt.Fprintf(out, "Report: %s (%.0f KB)\n", outPath, float64(st.Size())/1024)
	}

	if f.failOnDups && (c.ADupKeys > 0 || c.BDupKeys > 0) {
		return 3, nil
	}
	if result.Identical() {
		return 0, nil
	}
	return 1, nil
}

func stem(p string) string {
	name := filepath.Base(p)
	return strings.TrimSuffix(name, filepath.Ext(name))
}

// comma groups a count with thousands separators, matching the other implementations.
func comma(n int64) string {
	s := fmt.Sprint(n)
	sign := ""
	if strings.HasPrefix(s, "-") {
		sign, s = "-", s[1:]
	}
	var parts []string
	for len(s) > 3 {
		parts = append([]string{s[len(s)-3:]}, parts...)
		s = s[:len(s)-3]
	}
	return sign + strings.Join(append([]string{s}, parts...), ",")
}

// reportPeakRSS prints this process's peak resident set size for the benchmark
// harness, when it asks. Go has no portable API for it, so this reads VmHWM
// from /proc; on a platform without it nothing is printed and the harness
// records no figure rather than a wrong one.
func reportPeakRSS(errOut io.Writer) {
	if os.Getenv("CSVDIFF_PRINT_PEAK_RSS") == "" {
		return
	}
	data, err := os.ReadFile("/proc/self/status")
	if err != nil {
		return
	}
	for line := range strings.Lines(string(data)) {
		if kb, ok := strings.CutPrefix(line, "VmHWM:"); ok {
			kb = strings.TrimSuffix(strings.TrimSpace(kb), " kB")
			fmt.Fprintln(errOut, "PEAK_RSS_KB", strings.TrimSpace(kb))
			return
		}
	}
}
