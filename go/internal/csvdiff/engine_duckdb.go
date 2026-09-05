package csvdiff

import (
	"database/sql"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	_ "github.com/marcboeker/go-duckdb/v2"
)

// compareDuckDB runs the comparison inside DuckDB. The default, and the only
// engine here not bounded by the heap.
//
// Both files are read as text, normalised, de-duplicated on the key so the
// first occurrence joins, then full-outer-joined; only the differing cells come
// back into Go. DuckDB hash-joins in parallel and spills to disk, so files
// larger than memory are fine.
//
// SQL is assembled by string interpolation, so identifiers go through q() and
// string literals and paths through lit(). Nothing else is interpolated.
func compareDuckDB(aPath, bPath string, opt Options) (EngineResult, error) {
	db, err := sql.Open("duckdb", "")
	if err != nil {
		return EngineResult{}, fmt.Errorf("cannot start duckdb: %w", err)
	}
	defer db.Close()

	exec := func(query string) error {
		if _, err := db.Exec(query); err != nil {
			return fmt.Errorf("%w", err)
		}
		return nil
	}

	if opt.Threads > 0 {
		if err := exec("SET threads = " + strconv.Itoa(opt.Threads)); err != nil {
			return EngineResult{}, err
		}
	}
	if opt.MemoryLimit != "" {
		if err := exec("SET memory_limit = " + lit(opt.MemoryLimit)); err != nil {
			return EngineResult{}, err
		}
	}
	if err := exec("SET preserve_insertion_order = true"); err != nil {
		return EngineResult{}, err
	}

	// null_padding leaves a short row's missing fields absent, which is what the other
	// engines do with them. Without it DuckDB abandons the split on a ragged file and
	// returns it as one column named after the header line, and the comparison then fails
	// with "key column missing" -- a true statement about DuckDB's table and a misleading
	// one about the file.
	readOpts := "all_varchar=true, header=true, sample_size=-1, null_padding=true"
	if opt.Delimiter != "" {
		readOpts += ", delim=" + lit(opt.Delimiter)
	}
	if e := strings.ToLower(opt.Encoding); e != "" && e != "utf-8" && e != "utf8" {
		readOpts += ", encoding=" + lit(opt.Encoding)
	}

	load := func(tbl, path string) ([]string, error) {
		abs, err := filepath.Abs(path)
		if err != nil {
			return nil, err
		}
		if err := exec(fmt.Sprintf("CREATE TABLE %s_raw AS SELECT * FROM read_csv(%s, %s)", tbl, lit(abs), readOpts)); err != nil {
			return nil, err
		}
		rows, err := db.Query("DESCRIBE " + tbl + "_raw")
		if err != nil {
			return nil, err
		}
		defer rows.Close()
		return firstColumn(rows)
	}

	aCols, err := load("a", aPath)
	if err != nil {
		return EngineResult{}, err
	}
	bCols, err := load("b", bPath)
	if err != nil {
		return EngineResult{}, err
	}
	resolved, err := ResolveColumns(aCols, bCols, opt)
	if err != nil {
		return EngineResult{}, err
	}
	key, compared := opt.Key, resolved.Compared
	keySize := len(key)

	// Normalised projection plus a stable row number, so "first occurrence wins"
	// is well defined.
	for _, tbl := range []string{"a", "b"} {
		proj := make([]string, 0, keySize+len(compared))
		for _, c := range append(append([]string{}, key...), compared...) {
			proj = append(proj, norm(q(c), opt)+" AS "+q(c))
		}
		if err := exec(fmt.Sprintf("CREATE TABLE %s AS SELECT %s, row_number() OVER () AS _rn FROM %s_raw",
			tbl, strings.Join(proj, ", "), tbl)); err != nil {
			return EngineResult{}, err
		}
		if err := exec("DROP TABLE " + tbl + "_raw"); err != nil {
			return EngineResult{}, err
		}
	}

	kq := quotedList(key)
	counts := Counts{}
	if counts.ARows, err = scalar(db, "SELECT count(*) FROM a"); err != nil {
		return EngineResult{}, err
	}
	if counts.BRows, err = scalar(db, "SELECT count(*) FROM b"); err != nil {
		return EngineResult{}, err
	}

	dups := make([]Section, 2)
	totals := []int64{counts.ARows, counts.BRows}
	dupKeys := make([]int64, 2)
	dupRows := make([]int64, 2)
	for i, tbl := range []string{"a", "b"} {
		if err := exec(fmt.Sprintf(
			"CREATE TABLE %s_dup AS SELECT %s, count(*) AS n FROM %s GROUP BY ALL HAVING count(*) > 1", tbl, kq, tbl)); err != nil {
			return EngineResult{}, err
		}
		if dupKeys[i], err = scalar(db, "SELECT count(*) FROM "+tbl+"_dup"); err != nil {
			return EngineResult{}, err
		}
		if dupRows[i], err = scalar(db, "SELECT coalesce(sum(n), 0) FROM "+tbl+"_dup"); err != nil {
			return EngineResult{}, err
		}

		rows, err := db.Query(fmt.Sprintf("SELECT * FROM %s_dup ORDER BY n DESC, %s LIMIT %d", tbl, kq, opt.MaxRows))
		if err != nil {
			return EngineResult{}, err
		}
		section, err := scanRows(rows, keySize+1, keySize)
		if err != nil {
			return EngineResult{}, err
		}
		dups[i] = Section{
			Cols:      append(append([]string{}, key...), "count"),
			Rows:      section,
			Truncated: dupKeys[i] > int64(opt.MaxRows),
		}

		if err := exec(fmt.Sprintf(
			"CREATE TABLE %s1 AS SELECT * EXCLUDE(_k) FROM (SELECT *, row_number() OVER (PARTITION BY %s ORDER BY _rn) AS _k FROM %s) WHERE _k = 1",
			tbl, kq, tbl)); err != nil {
			return EngineResult{}, err
		}
	}
	counts.ADupKeys, counts.ADupRows = dupKeys[0], dupRows[0]
	counts.BDupKeys, counts.BDupRows = dupKeys[1], dupRows[1]
	counts.AKeys = totals[0] - dupRows[0] + dupKeys[0]
	counts.BKeys = totals[1] - dupRows[1] + dupKeys[1]

	if err := buildJoin(exec, key, compared, opt); err != nil {
		return EngineResult{}, err
	}

	statusRows, err := db.Query("SELECT _status, _changed, count(*) FROM j GROUP BY ALL")
	if err != nil {
		return EngineResult{}, err
	}
	for statusRows.Next() {
		var status string
		var isChanged bool
		var n int64
		if err := statusRows.Scan(&status, &isChanged, &n); err != nil {
			statusRows.Close()
			return EngineResult{}, err
		}
		switch status {
		case "matched":
			if isChanged {
				counts.Changed += n
			} else {
				counts.Unchanged += n
			}
		case "added":
			counts.Added += n
		case "removed":
			counts.Removed += n
		}
	}
	statusRows.Close()
	counts.Matched = counts.Unchanged + counts.Changed

	columns, err := columnStats(db, compared)
	if err != nil {
		return EngineResult{}, err
	}
	changed, err := changedRows(db, key, compared, opt, counts)
	if err != nil {
		return EngineResult{}, err
	}
	added, err := sideRows(db, "added", "b_", key, compared, opt, counts)
	if err != nil {
		return EngineResult{}, err
	}
	removed, err := sideRows(db, "removed", "a_", key, compared, opt, counts)
	if err != nil {
		return EngineResult{}, err
	}

	if opt.ExportDir != "" {
		if err := exportSQL(exec, key, compared, opt); err != nil {
			return EngineResult{}, err
		}
	}

	return EngineResult{
		Meta:    resolved.Meta(key, len(aCols), len(bCols)),
		Counts:  counts,
		Columns: columns,
		Changed: changed,
		Added:   added,
		Removed: removed,
		DupA:    dups[0],
		DupB:    dups[1],
	}, nil
}

func buildJoin(exec func(string) error, key, compared []string, opt Options) error {
	keySel := make([]string, 0, len(key))
	on := make([]string, 0, len(key))
	for _, k := range key {
		keySel = append(keySel, fmt.Sprintf("coalesce(a.%s, b.%s) AS %s", q(k), q(k), q(k)))
		on = append(on, fmt.Sprintf("a.%s IS NOT DISTINCT FROM b.%s", q(k), q(k)))
	}
	colSel := make([]string, 0, 3*len(compared))
	flags := make([]string, 0, len(compared))
	for i, c := range compared {
		a, b := "a."+q(c), "b."+q(c)
		d := diffExpr(a, b, opt)
		colSel = append(colSel, a+" AS "+q("a_"+strconv.Itoa(i)), b+" AS "+q("b_"+strconv.Itoa(i)), d+" AS "+q("d_"+strconv.Itoa(i)))
		flags = append(flags, d)
	}
	changedExpr := "false"
	if len(flags) > 0 {
		changedExpr = strings.Join(flags, " OR ")
	}
	cols := ""
	if len(colSel) > 0 {
		cols = strings.Join(colSel, ", ") + ", "
	}
	return exec(fmt.Sprintf(`CREATE TABLE j AS SELECT %s,
		CASE WHEN a._rn IS NULL THEN 'added' WHEN b._rn IS NULL THEN 'removed' ELSE 'matched' END AS _status,
		%s(%s) AS _changed
		FROM a1 a FULL OUTER JOIN b1 b ON %s`,
		strings.Join(keySel, ", "), cols, changedExpr, strings.Join(on, " AND ")))
}

func columnStats(db *sql.DB, compared []string) ([]ColumnStat, error) {
	if len(compared) == 0 {
		return []ColumnStat{}, nil
	}
	agg := make([]string, 0, 3*len(compared))
	for i := range compared {
		s := strconv.Itoa(i)
		agg = append(agg,
			"coalesce(sum("+q("d_"+s)+"::INT), 0)",
			"coalesce(sum(("+q("a_"+s)+" IS NOT NULL AND "+q("b_"+s)+" IS NULL)::INT), 0)",
			"coalesce(sum(("+q("a_"+s)+" IS NULL AND "+q("b_"+s)+" IS NOT NULL)::INT), 0)")
	}
	row := db.QueryRow("SELECT " + strings.Join(agg, ", ") + " FROM j WHERE _status = 'matched'")
	vals := make([]int64, 3*len(compared))
	dest := make([]any, len(vals))
	for i := range vals {
		dest[i] = &vals[i]
	}
	if err := row.Scan(dest...); err != nil {
		return nil, err
	}
	out := make([]ColumnStat, len(compared))
	for i, c := range compared {
		out[i] = ColumnStat{Name: c, Changed: vals[3*i], Blanked: vals[3*i+1], Filled: vals[3*i+2]}
	}
	return out, nil
}

func changedRows(db *sql.DB, key, compared []string, opt Options, counts Counts) (Section, error) {
	keySize := len(key)
	rows := [][]any{}
	if len(compared) > 0 {
		sel := []string{quotedList(key)}
		for i := range compared {
			s := strconv.Itoa(i)
			sel = append(sel, q("a_"+s)+", "+q("b_"+s)+", "+q("d_"+s))
		}
		query := fmt.Sprintf("SELECT %s FROM j WHERE _status = 'matched' AND _changed ORDER BY %s LIMIT %d",
			strings.Join(sel, ", "), quotedList(key), opt.MaxRows)
		rs, err := db.Query(query)
		if err != nil {
			return Section{}, err
		}
		defer rs.Close()

		width := keySize + 3*len(compared)
		for rs.Next() {
			holders := make([]any, width)
			keys := make([]sql.NullString, keySize)
			cells := make([]sql.NullString, 2*len(compared))
			flags := make([]sql.NullBool, len(compared))
			for i := range keySize {
				holders[i] = &keys[i]
			}
			for i := range compared {
				holders[keySize+3*i] = &cells[2*i]
				holders[keySize+3*i+1] = &cells[2*i+1]
				holders[keySize+3*i+2] = &flags[i]
			}
			if err := rs.Scan(holders...); err != nil {
				return Section{}, err
			}
			row := make([]any, 0, keySize+1)
			for i := range keySize {
				row = append(row, nullable(keys[i]))
			}
			var diffs []CellDiff
			for i := range compared {
				if flags[i].Valid && flags[i].Bool {
					diffs = append(diffs, CellDiff{Column: i, A: nullable(cells[2*i]), B: nullable(cells[2*i+1])})
				}
			}
			rows = append(rows, append(row, diffs))
		}
		if err := rs.Err(); err != nil {
			return Section{}, err
		}
	}
	return Section{Cols: key, Rows: rows, Truncated: counts.Changed > int64(opt.MaxRows)}, nil
}

func sideRows(db *sql.DB, status, prefix string, key, compared []string, opt Options, counts Counts) (Section, error) {
	sel := []string{quotedList(key)}
	for i := range compared {
		sel = append(sel, q(prefix+strconv.Itoa(i)))
	}
	query := fmt.Sprintf("SELECT %s FROM j WHERE _status = %s ORDER BY %s LIMIT %d",
		strings.Join(sel, ", "), lit(status), quotedList(key), opt.MaxRows)
	rs, err := db.Query(query)
	if err != nil {
		return Section{}, err
	}
	width := len(key) + len(compared)
	rows, err := scanRows(rs, width, width)
	if err != nil {
		return Section{}, err
	}
	return Section{
		Cols:      append(append([]string{}, key...), compared...),
		Rows:      rows,
		Truncated: counts.ForStatus(status) > int64(opt.MaxRows),
	}, nil
}

func exportSQL(exec func(string) error, key, compared []string, opt Options) error {
	dir := opt.ExportDir
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return fmt.Errorf("cannot create export-dir %s: %w", dir, err)
	}
	kq := quotedList(key)
	aliases := func(prefix string) string {
		var sb strings.Builder
		for i, c := range compared {
			sb.WriteString(", " + q(prefix+strconv.Itoa(i)) + " AS " + q(c))
		}
		return sb.String()
	}
	if err := exec(fmt.Sprintf("COPY (SELECT %s%s FROM j WHERE _status='added' ORDER BY %s) TO %s (HEADER)",
		kq, aliases("b_"), kq, lit(filepath.Join(dir, "added.csv")))); err != nil {
		return err
	}
	if err := exec(fmt.Sprintf("COPY (SELECT %s%s FROM j WHERE _status='removed' ORDER BY %s) TO %s (HEADER)",
		kq, aliases("a_"), kq, lit(filepath.Join(dir, "removed.csv")))); err != nil {
		return err
	}
	if len(compared) > 0 {
		var both strings.Builder
		for i, c := range compared {
			s := strconv.Itoa(i)
			both.WriteString(", " + q("a_"+s) + " AS " + q(c+" (A)"))
			both.WriteString(", " + q("b_"+s) + " AS " + q(c+" (B)"))
		}
		if err := exec(fmt.Sprintf("COPY (SELECT %s%s FROM j WHERE _status='matched' AND _changed ORDER BY %s) TO %s (HEADER)",
			kq, both.String(), kq, lit(filepath.Join(dir, "changed.csv")))); err != nil {
			return err
		}
	}
	return nil
}

// scanRows reads `width` columns, treating the first `stringCols` as text and
// anything after that as an integer (the duplicate-key count).
func scanRows(rs *sql.Rows, width, stringCols int) ([][]any, error) {
	defer rs.Close()
	out := [][]any{}
	for rs.Next() {
		strs := make([]sql.NullString, stringCols)
		nums := make([]sql.NullInt64, width-stringCols)
		holders := make([]any, width)
		for i := range stringCols {
			holders[i] = &strs[i]
		}
		for i := range width - stringCols {
			holders[stringCols+i] = &nums[i]
		}
		if err := rs.Scan(holders...); err != nil {
			return nil, err
		}
		row := make([]any, 0, width)
		for i := range stringCols {
			row = append(row, nullable(strs[i]))
		}
		for i := range width - stringCols {
			row = append(row, nums[i].Int64)
		}
		out = append(out, row)
	}
	return out, rs.Err()
}

func nullable(v sql.NullString) *string {
	if !v.Valid {
		return nil
	}
	s := v.String
	return &s
}

func firstColumn(rows *sql.Rows) ([]string, error) {
	defer rows.Close()
	cols, err := rows.Columns()
	if err != nil {
		return nil, err
	}
	var out []string
	for rows.Next() {
		holders := make([]any, len(cols))
		var name string
		holders[0] = &name
		for i := 1; i < len(cols); i++ {
			holders[i] = new(sql.RawBytes)
		}
		if err := rows.Scan(holders...); err != nil {
			return nil, err
		}
		out = append(out, name)
	}
	return out, rows.Err()
}

func scalar(db *sql.DB, query string) (int64, error) {
	var n sql.NullInt64
	if err := db.QueryRow(query).Scan(&n); err != nil {
		return 0, err
	}
	return n.Int64, nil
}

func norm(expr string, opt Options) string {
	if opt.Trim {
		expr = "trim(" + expr + ")"
	}
	if opt.IgnoreCase {
		expr = "lower(" + expr + ")"
	}
	if opt.EmptyIsNull {
		expr = "nullif(" + expr + ", '')"
	}
	return expr
}

func diffExpr(a, b string, opt Options) string {
	if opt.Tolerance > 0 {
		tol := strconv.FormatFloat(opt.Tolerance, 'g', -1, 64)
		return fmt.Sprintf("(CASE WHEN try_cast(%s AS DOUBLE) IS NOT NULL AND try_cast(%s AS DOUBLE) IS NOT NULL "+
			"THEN abs(try_cast(%s AS DOUBLE) - try_cast(%s AS DOUBLE)) > %s ELSE (%s IS DISTINCT FROM %s) END)",
			a, b, a, b, tol, a, b)
	}
	return "(" + a + " IS DISTINCT FROM " + b + ")"
}

func quotedList(names []string) string {
	out := make([]string, len(names))
	for i, n := range names {
		out[i] = q(n)
	}
	return strings.Join(out, ", ")
}

// q quotes a SQL identifier.
func q(name string) string { return `"` + strings.ReplaceAll(name, `"`, `""`) + `"` }

// lit quotes a SQL string literal.
func lit(s string) string { return "'" + strings.ReplaceAll(s, "'", "''") + "'" }

// duckDBAvailable reports whether the bundled DuckDB library can actually open a
// database here. The driver is linked in, but the shared library it needs may
// not load on every platform, so this probes rather than assumes.
func duckDBAvailable() bool {
	db, err := sql.Open("duckdb", "")
	if err != nil {
		return false
	}
	defer db.Close()
	return db.Ping() == nil
}
