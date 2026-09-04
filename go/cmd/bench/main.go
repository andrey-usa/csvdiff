// Command bench benchmarks one scale end to end and records the numbers.
//
//	go run ./cmd/bench --rows 1m --engine duckdb --out-dir bench
//
// The comparison runs in a child process so peak RSS is measured honestly
// rather than including this process's own heap. Writes
// bench/<scale>-<engine>.json, appends a row to $GITHUB_STEP_SUMMARY under
// Actions, and exits non-zero when a budget is exceeded. An engine that cannot
// handle a scale is recorded as a failed row with the reason rather than
// aborting the run, because where an engine stops is a result worth having.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"math"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"time"
)

const (
	keyColumns    = "account_id,txn_id"
	ignoreColumns = "updated_at"
)

// budget is the wall-clock and peak-RSS allowance for a scale, on a 4-vCPU /
// 16 GB GitHub-hosted runner, generous by about 2x.
type budget struct {
	rows    int64
	seconds float64
	peakMB  float64
}

var budgets = []budget{
	{10_000, 20, 1_500},
	{1_000_000, 120, 6_000},
	{10_000_000, 900, 12_000},
}

func budgetFor(rows int64) budget {
	for _, b := range budgets {
		if rows <= b.rows {
			return b
		}
	}
	return budgets[len(budgets)-1]
}

type config struct {
	rowsArg, label, engine string
	outDir, dataDir        string
	binary                 string
	keepData, noBudget     bool
	allowFailure           bool
	threads                int
	memoryLimit            string
	timeout                time.Duration
}

func main() {
	cfg := parseConfig()
	if err := run(cfg); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(2)
	}
}

func parseConfig() config {
	var cfg config
	flag.StringVar(&cfg.rowsArg, "rows", "10k", "scale, e.g. 10k, 1m")
	flag.StringVar(&cfg.rowsArg, "n", "10k", "shorthand for --rows")
	flag.StringVar(&cfg.engine, "engine", "auto", "engine to benchmark")
	flag.StringVar(&cfg.outDir, "out-dir", "bench", "where results are written")
	flag.StringVar(&cfg.outDir, "o", "bench", "shorthand for --out-dir")
	flag.StringVar(&cfg.dataDir, "data-dir", "data", "where the generated CSVs live")
	flag.StringVar(&cfg.binary, "binary", "", "csvdiff binary to run (default: build one)")
	flag.IntVar(&cfg.threads, "threads", 0, "engine threads, 0 for the default")
	flag.StringVar(&cfg.memoryLimit, "memory-limit", "", "DuckDB memory limit, e.g. 4GB")
	flag.BoolVar(&cfg.keepData, "keep-data", false, "keep the generated CSVs")
	flag.BoolVar(&cfg.noBudget, "no-budget", false, "report the budget but always exit 0")
	flag.BoolVar(&cfg.allowFailure, "allow-failure", false, "exit 0 even when the comparison fails")
	flag.DurationVar(&cfg.timeout, "timeout", 6*time.Hour, "give up on the child after this long")
	flag.Parse()
	cfg.label = strings.ToLower(cfg.rowsArg)
	return cfg
}

func run(cfg config) error {
	rows, err := parseRows(cfg.rowsArg)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(cfg.outDir, 0o755); err != nil {
		return err
	}
	if err := os.MkdirAll(cfg.dataDir, 0o755); err != nil {
		return err
	}

	aPath := filepath.Join(cfg.dataDir, cfg.label+"_a.csv")
	bPath := filepath.Join(cfg.dataDir, cfg.label+"_b.csv")
	genSeconds, err := ensureData(cfg, rows, aPath, bPath)
	if err != nil {
		return err
	}

	binary, cleanup, err := ensureBinary(cfg)
	if err != nil {
		return err
	}
	defer cleanup()

	report := filepath.Join(cfg.outDir, cfg.label+"-"+cfg.engine+".html")
	summary := filepath.Join(cfg.outDir, cfg.label+"-"+cfg.engine+"-summary.json")
	_ = os.Remove(summary) // never read a stale summary as this run's result

	args := []string{"compare", aPath, bPath, "-k", keyColumns, "-i", ignoreColumns,
		"--engine", cfg.engine, "-o", report, "--json", summary}
	if cfg.threads > 0 {
		args = append(args, "--threads", strconv.Itoa(cfg.threads))
	}
	if cfg.memoryLimit != "" {
		args = append(args, "--memory-limit", cfg.memoryLimit)
	}

	status, stderr, wall := runChild(binary, args, cfg.timeout)
	peakMB := round(float64(peakRSSKB(stderr))/1024, 1)
	inputMB := round(float64(fileSize(aPath)+fileSize(bPath))/1e6, 1)
	env := runnerDescription()

	fail := func(why string) error {
		if err := recordFailure(cfg, rows, genSeconds, inputMB, peakMB, wall, status, why, env); err != nil {
			return err
		}
		fmt.Fprintf(os.Stderr, "compare exited with %d: %s\n", status, why)
		cleanData(cfg, aPath, bPath)
		if cfg.allowFailure {
			os.Exit(0)
		}
		os.Exit(2)
		return nil
	}

	if status != 0 && status != 1 {
		return fail(classify(status, stderr))
	}
	counts, err := readCounts(summary)
	if err != nil {
		// The child reported success but wrote nothing usable. Record it as a failure
		// rather than crashing the harness, and never present it as a result.
		return fail("no summary written")
	}

	reportMB := round(float64(fileSize(report))/1e6, 2)
	var perSecond int64
	if wall > 0 {
		perSecond = int64(math.Round(float64(counts["a_rows"]+counts["b_rows"]) / wall))
	}

	rec := map[string]any{
		"rows": rows, "scale": cfg.label, "engine": cfg.engine,
		"generate_seconds": genSeconds, "compare_seconds": wall,
		"peak_rss_mb": peakMB, "input_mb": inputMB, "report_mb": reportMB,
		"rows_per_second": perSecond, "counts": counts, "runner": env,
	}
	if err := writeJSON(filepath.Join(cfg.outDir, cfg.label+"-"+cfg.engine+".json"), rec); err != nil {
		return err
	}

	b := budgetFor(rows)
	ok := wall <= b.seconds && peakMB <= b.peakMB
	verdict := "pass"
	if !ok {
		verdict = fmt.Sprintf("over budget (%.0fs / %.0f MB)", b.seconds, b.peakMB)
	}
	throughput := "-"
	if perSecond > 0 {
		throughput = comma(perSecond) + "/s"
	}
	line := fmt.Sprintf("| %s | %s | %s MB | %gs | **%gs** | %s | %s MB | %g MB | %s | %s | %s | %s |",
		cfg.label, cfg.engine, num(inputMB), genSeconds, wall, throughput, num(peakMB), reportMB,
		comma(counts["changed"]), comma(counts["added"]), comma(counts["removed"]), verdict)
	appendSummary(line)
	fmt.Println(line)

	cleanData(cfg, aPath, bPath)
	if ok || cfg.noBudget {
		return nil
	}
	os.Exit(1)
	return nil
}

// ensureData generates the pair of CSVs unless they are already there.
func ensureData(cfg config, rows int64, aPath, bPath string) (float64, error) {
	if fileSize(aPath) > 0 && fileSize(bPath) > 0 {
		return 0, nil
	}
	start := time.Now()
	cmd := exec.Command("go", "run", "./cmd/gen-data", "--rows", cfg.rowsArg,
		"--out-dir", cfg.dataDir, "--prefix", cfg.label)
	cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
	if err := cmd.Run(); err != nil {
		return 0, fmt.Errorf("cannot generate data: %w", err)
	}
	seconds := round(time.Since(start).Seconds(), 1)
	fmt.Printf("go: generated %s rows x 20 columns in %.1fs\n", comma(rows), seconds)
	return seconds, nil
}

// ensureBinary builds the CLI once, so the compile time is not counted as
// comparison time the way `go run` would count it.
func ensureBinary(cfg config) (string, func(), error) {
	if cfg.binary != "" {
		return cfg.binary, func() {}, nil
	}
	dir, err := os.MkdirTemp("", "csvdiff-bench")
	if err != nil {
		return "", func() {}, err
	}
	binary := filepath.Join(dir, "csvdiff")
	cmd := exec.Command("go", "build", "-o", binary, "./cmd/csvdiff")
	cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
	if err := cmd.Run(); err != nil {
		os.RemoveAll(dir)
		return "", func() {}, fmt.Errorf("cannot build csvdiff: %w", err)
	}
	return binary, func() { os.RemoveAll(dir) }, nil
}

// runChild runs the comparison and returns its exit status, stderr and wall time.
func runChild(binary string, args []string, timeout time.Duration) (int, string, float64) {
	cmd := exec.Command(binary, args...)
	cmd.Stdout = os.Stdout
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	// The child reports its own peak RSS on stderr, so the figure excludes this process.
	cmd.Env = append(os.Environ(), "CSVDIFF_PRINT_PEAK_RSS=1")

	start := time.Now()
	if err := cmd.Start(); err != nil {
		return -2, err.Error(), 0
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()

	status := 0
	select {
	case err := <-done:
		if err != nil {
			status = cmd.ProcessState.ExitCode()
		}
	case <-time.After(timeout):
		_ = cmd.Process.Kill()
		<-done
		status = -1
	}
	wall := round(time.Since(start).Seconds(), 2)
	os.Stderr.WriteString(stderr.String())
	return status, stderr.String(), wall
}

// recordFailure records a scale an engine could not handle: a JSON row plus a
// line in the results table.
func recordFailure(cfg config, rows int64, genSeconds, inputMB, peakMB, wall float64,
	status int, why, env string) error {
	rec := map[string]any{
		"rows": rows, "scale": cfg.label, "engine": cfg.engine,
		"generate_seconds": genSeconds, "compare_seconds": nil,
		"peak_rss_mb": nil, "input_mb": inputMB, "report_mb": nil,
		"rows_per_second": nil, "counts": nil,
		"failed": why, "exit_status": status, "wall_before_failure": wall, "runner": env,
	}
	if peakMB > 0 {
		rec["peak_rss_mb"] = peakMB
	}
	if err := writeJSON(filepath.Join(cfg.outDir, cfg.label+"-"+cfg.engine+".json"), rec); err != nil {
		return err
	}
	peak := "-"
	if peakMB > 0 {
		peak = num(peakMB) + " MB"
	}
	line := fmt.Sprintf("| %s | %s | %s MB | %gs | **%s** after %gs | - | %s | - | - | - | - | failed |",
		cfg.label, cfg.engine, num(inputMB), genSeconds, why, wall, peak)
	appendSummary(line)
	fmt.Println(line)
	return nil
}

// classify turns a dead child process into a short honest reason for the results table.
func classify(status int, stderr string) string {
	switch {
	case strings.Contains(stderr, "out of memory"),
		strings.Contains(stderr, "cannot allocate memory"),
		strings.Contains(stderr, "runtime: out of memory"):
		return "Go heap OOM"
	case strings.Contains(stderr, "No space left on device"):
		return "disk full"
	}
	switch status {
	case 137:
		return "OOM killed"
	case 139:
		return "segfault"
	case -1:
		return "timed out"
	default:
		return "exit " + strconv.Itoa(status)
	}
}

func readCounts(path string) (map[string]int64, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, err
	}
	var summary struct {
		Counts map[string]int64 `json:"counts"`
	}
	if err := json.Unmarshal(data, &summary); err != nil {
		return nil, err
	}
	if summary.Counts == nil {
		return nil, fmt.Errorf("%s has no counts", path)
	}
	return summary.Counts, nil
}

func peakRSSKB(stderr string) int64 {
	var peak int64
	for line := range strings.Lines(stderr) {
		if rest, ok := strings.CutPrefix(strings.TrimSpace(line), "PEAK_RSS_KB"); ok {
			if v, err := strconv.ParseInt(strings.TrimSpace(rest), 10, 64); err == nil && v > peak {
				peak = v
			}
		}
	}
	return peak
}

func runnerDescription() string {
	return fmt.Sprintf("%s %s %s %d cpu", runtime.GOOS, runtime.GOARCH, runtime.Version(), runtime.NumCPU())
}

func cleanData(cfg config, paths ...string) {
	if cfg.keepData {
		return
	}
	for _, p := range paths {
		_ = os.Remove(p)
	}
}

func writeJSON(path string, rec map[string]any) error {
	data, err := json.MarshalIndent(rec, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(path, append(data, '\n'), 0o644)
}

func appendSummary(line string) {
	step := os.Getenv("GITHUB_STEP_SUMMARY")
	if step == "" {
		return
	}
	f, err := os.OpenFile(step, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return
	}
	defer f.Close()
	if st, err := f.Stat(); err == nil && st.Size() == 0 {
		fmt.Fprint(f, "| scale | engine | input | generate | compare | throughput | peak RSS | report "+
			"| changed | added | removed | budget |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n")
	}
	fmt.Fprintln(f, line)
}

func fileSize(p string) int64 {
	st, err := os.Stat(p)
	if err != nil {
		return 0
	}
	return st.Size()
}

func round(v float64, decimals int) float64 {
	f := math.Pow(10, float64(decimals))
	return math.Round(v*f) / f
}

// num prints a whole number without a pointless ".0", and one decimal otherwise.
func num(v float64) string {
	if v == math.Trunc(v) {
		return comma(int64(v))
	}
	return strconv.FormatFloat(v, 'f', 1, 64)
}

func comma(n int64) string {
	s := strconv.FormatInt(n, 10)
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

// parseRows accepts 10000, 10k, 1m, 2.5M.
func parseRows(s string) (int64, error) {
	t := strings.NewReplacer("_", "", ",", "").Replace(strings.ToLower(strings.TrimSpace(s)))
	if t == "" {
		return 0, fmt.Errorf("--rows must not be empty")
	}
	mult := int64(0)
	switch t[len(t)-1] {
	case 'k':
		mult = 1_000
	case 'm':
		mult = 1_000_000
	case 'g':
		mult = 1_000_000_000
	}
	if mult == 0 {
		return strconv.ParseInt(t, 10, 64)
	}
	f, err := strconv.ParseFloat(t[:len(t)-1], 64)
	if err != nil {
		return 0, fmt.Errorf("--rows must be a number, got: %s", s)
	}
	return int64(math.Round(f * float64(mult))), nil
}
