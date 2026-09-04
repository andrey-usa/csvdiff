// Command gen-data writes a deterministic pair of CSV files for benchmarking and CI.
//
// Both files have 20 columns and share the composite key (account_id, txn_id).
// File B is file A with a controlled amount of drift, so every run has a known
// answer:
//
//	status changed         3.0%
//	amount changed         1.5%
//	balance changed        1.5%
//	value_date blanked     0.3%
//	updated_at changed     100%   (excluded with --ignore)
//	rows only in B         0.10%
//	rows only in A         0.10%
//	duplicate keys         0.01% per file
//
// The recipe and the hash are the same as the Python, TypeScript and Java
// generators — the files come out byte for byte identical — so a benchmark
// number from any of them is directly comparable.
package main

import (
	"bufio"
	"flag"
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

// Columns is the 20-column schema, in order.
var Columns = []string{
	"account_id", "txn_id", "posting_date", "value_date", "currency", "amount", "fee",
	"balance", "status", "channel", "region", "branch_code", "product_code", "counterparty",
	"quantity", "rate", "category", "risk_flag", "note", "updated_at",
}

var (
	status   = []string{"posted", "pending", "settled", "reversed"}
	channel  = []string{"branch", "online", "mobile", "atm", "wire"}
	region   = []string{"EMEA", "NA", "APAC", "LATAM"}
	currency = []string{"USD", "EUR", "GBP", "JPY"}
	category = []string{"retail", "corporate", "treasury", "cards", "loans"}
)

// Drift buckets, against a 0..9999 hash bucket per row.
const (
	chgStatus    = 300
	chgAmount    = 150
	chgBalance   = 150
	chgValueDate = 30
	removedMod   = 1000
	addedRatio   = 1000
	dupMod       = 10_000
)

var days = buildDays()

func buildDays() []string {
	out := make([]string, 240)
	d0 := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	for i := range out {
		out[i] = d0.AddDate(0, 0, i).Format("2006-01-02")
	}
	return out
}

// hash is a splitmix-style mix, matching the other implementations bit for bit.
func hash(i, salt, seed int64) uint64 {
	x := uint64(i*31 + salt + seed)
	x = (x ^ (x >> 30)) * 0xBF58476D1CE4E5B9
	x = (x ^ (x >> 27)) * 0x94D049BB133111EB
	return x ^ (x >> 31)
}

func mod(i, salt, seed int64, m int) int {
	return int(hash(i, salt, seed) % uint64(m))
}

// row builds one row of one side.
//
// Money is carried in integer cents and the drift is applied to those integers,
// never to a float. Every implementation of this generator then produces the
// same digits without depending on its language's rounding rule — which is what
// makes the five sets of files byte-identical.
func row(sb *strings.Builder, i int64, b bool, seed int64) {
	bucket := mod(i, 0, seed, 10_000)
	amountCents := int64(mod(i, 21, seed, 900_000_000)) - 100_000_000
	balanceCents := int64(mod(i, 31, seed, 2_000_000_000))
	st := status[mod(i, 11, seed, len(status))]
	valueDate := days[mod(i, 41, seed, 240)]

	if b {
		switch {
		case bucket < chgStatus:
			st = status[(mod(i, 11, seed, len(status))+1)%len(status)]
		case bucket < chgStatus+chgAmount:
			amountCents += 1234
		case bucket < chgStatus+chgAmount+chgBalance:
			// +1%, rounded half up, in cents.
			balanceCents = (balanceCents*101 + 50) / 100
		}
		if bucket < chgValueDate {
			valueDate = ""
		}
	}

	sb.Reset()
	sb.WriteString("ACC-")
	pad(sb, (i*7919)%250_000, 8)
	sb.WriteString(",TXN-")
	pad(sb, i, 11)
	sb.WriteByte(',')
	sb.WriteString(days[mod(i, 1, seed, 240)])
	sb.WriteByte(',')
	sb.WriteString(valueDate)
	sb.WriteByte(',')
	sb.WriteString(currency[mod(i, 51, seed, 4)])
	sb.WriteByte(',')
	money(sb, amountCents)
	sb.WriteByte(',')
	money(sb, int64(mod(i, 61, seed, 5000)))
	sb.WriteByte(',')
	money(sb, balanceCents)
	sb.WriteByte(',')
	sb.WriteString(st)
	sb.WriteByte(',')
	sb.WriteString(channel[mod(i, 71, seed, 5)])
	sb.WriteByte(',')
	sb.WriteString(region[mod(i, 81, seed, 4)])
	sb.WriteString(",BR")
	pad(sb, int64(mod(i, 91, seed, 900))+100, 4)
	sb.WriteString(",P")
	pad(sb, int64(mod(i, 101, seed, 5000)), 5)
	sb.WriteString(",CP-")
	pad(sb, int64(mod(i, 111, seed, 90_000)), 6)
	sb.WriteByte(',')
	sb.WriteString(strconv.Itoa(mod(i, 121, seed, 500) + 1))
	sb.WriteString(",0.")
	pad(sb, int64(mod(i, 131, seed, 1200)), 4)
	sb.WriteByte(',')
	sb.WriteString(category[mod(i, 141, seed, 5)])
	sb.WriteByte(',')
	if mod(i, 151, seed, 20) == 0 {
		sb.WriteByte('Y')
	} else {
		sb.WriteByte('N')
	}
	sb.WriteString(",batch ")
	sb.WriteString(strconv.FormatInt(i%997+1, 10))
	sb.WriteString(" line ")
	sb.WriteString(strconv.FormatInt(i%53+1, 10))
	sb.WriteByte(',')
	if b {
		sb.WriteString("2026-09-01 02:15:00")
	} else {
		sb.WriteString("2026-08-01 02:15:00")
	}
	sb.WriteByte('\n')
}

// money writes an amount held in cents as a two-decimal number.
func money(sb *strings.Builder, cents int64) {
	if cents < 0 {
		sb.WriteByte('-')
		cents = -cents
	}
	sb.WriteString(strconv.FormatInt(cents/100, 10))
	sb.WriteByte('.')
	pad(sb, cents%100, 2)
}

func pad(sb *strings.Builder, v int64, width int) {
	s := strconv.FormatInt(v, 10)
	for range width - len(s) {
		sb.WriteByte('0')
	}
	sb.WriteString(s)
}

// generate writes both files in one pass.
func generate(rows int64, aPath, bPath string, seed int64) error {
	fa, err := os.Create(aPath)
	if err != nil {
		return err
	}
	defer fa.Close()
	fb, err := os.Create(bPath)
	if err != nil {
		return err
	}
	defer fb.Close()

	wa := bufio.NewWriterSize(fa, 1<<20)
	wb := bufio.NewWriterSize(fb, 1<<20)
	header := strings.Join(Columns, ",") + "\n"
	wa.WriteString(header)
	wb.WriteString(header)

	dupExtra := max(int64(1), rows/dupMod)
	added := max(int64(1), rows/addedRatio)

	var sb strings.Builder
	sb.Grow(256)
	for i := range rows {
		row(&sb, i, false, seed)
		wa.WriteString(sb.String())
		if i%removedMod != 7 {
			row(&sb, i, true, seed)
			wb.WriteString(sb.String())
		}
		if i%dupMod == 3 && i < rows/2 {
			row(&sb, i, true, seed)
			wb.WriteString(sb.String())
		}
	}
	for i := range dupExtra {
		row(&sb, i, false, seed)
		wa.WriteString(sb.String())
	}
	for i := rows; i < rows+added; i++ {
		row(&sb, i, true, seed)
		wb.WriteString(sb.String())
	}
	if err := wa.Flush(); err != nil {
		return err
	}
	return wb.Flush()
}

// ParseRows accepts 10000, 10k, 1m, 2.5M.
func ParseRows(s string) (int64, error) {
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

func main() {
	rowsArg := flag.String("rows", "10k", "rows to generate, e.g. 10k, 1m")
	flag.StringVar(rowsArg, "n", "10k", "shorthand for --rows")
	outDir := flag.String("out-dir", "data", "directory to write into")
	flag.StringVar(outDir, "o", "data", "shorthand for --out-dir")
	prefix := flag.String("prefix", "", "file name prefix (default: the row count)")
	seed := flag.Int64("seed", 7, "hash seed")
	flag.Parse()

	rows, err := ParseRows(*rowsArg)
	if err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(2)
	}
	if err := os.MkdirAll(*outDir, 0o755); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(2)
	}
	p := *prefix
	if p == "" {
		p = strings.ToLower(*rowsArg)
	}
	aPath := filepath.Join(*outDir, p+"_a.csv")
	bPath := filepath.Join(*outDir, p+"_b.csv")

	start := time.Now()
	if err := generate(rows, aPath, bPath, *seed); err != nil {
		fmt.Fprintln(os.Stderr, "error:", err)
		os.Exit(2)
	}
	fmt.Printf("go: %d rows x %d columns in %.1fs\n", rows, len(Columns), time.Since(start).Seconds())
	for _, path := range []string{aPath, bPath} {
		if st, err := os.Stat(path); err == nil {
			fmt.Printf("  %s  %.1f MB\n", path, float64(st.Size())/1e6)
		}
	}
}
