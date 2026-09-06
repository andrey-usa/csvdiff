/*
 * The floor: the byte-level comparison in C, with nothing under it.
 *
 * The same design as the Java, Rust, C++ and Zig ports — map the file, pack a
 * field into one word, find delimiters eight bytes at a time, build no string
 * for a cell — written with a fixed arena and no library beyond libc.
 *
 * This port exists to answer one question the others cannot: how little memory
 * can a correct answer be had in? It is the baseline the rest are measured
 * against, not a recommendation.
 *
 * Build:  cc -std=c11 -O2 -march=native -Wall -Wextra -o csvdiff csvdiff.c
 * Usage:  csvdiff compare A B -k COLS [-i COLS] [--json PATH]
 * Exit:   0 identical, 1 differences found, 2 error
 */
#define _GNU_SOURCE
#include <ctype.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

/* ------------------------------------------------------------------------- */
/* A field packed into one word: offset, length, and whether it needs           */
/* unescaping. 40 bits of offset addresses a terabyte and 23 bits of length a   */
/* field of eight megabytes; an over-long field is reported, not truncated.     */
/* ------------------------------------------------------------------------- */

typedef uint64_t Field;

#define ABSENT       UINT64_MAX
#define TOO_LONG     (UINT64_MAX - 1)
#define OFFSET_MASK  ((UINT64_C(1) << 40) - 1)
#define LENGTH_SHIFT 40
#define LENGTH_MASK  ((UINT64_C(1) << 23) - 1)
#define ESCAPED_BIT  (UINT64_C(1) << 63)
#define MAX_FIELD    LENGTH_MASK

static Field pack(size_t off, size_t len, bool escaped) {
    if (len > MAX_FIELD) return TOO_LONG;
    return ((uint64_t)off & OFFSET_MASK) | (((uint64_t)len & LENGTH_MASK) << LENGTH_SHIFT) |
           (escaped ? ESCAPED_BIT : 0);
}
static size_t field_off(Field f) { return (size_t)(f & OFFSET_MASK); }
static size_t field_len(Field f) { return (size_t)((f >> LENGTH_SHIFT) & LENGTH_MASK); }
static bool field_escaped(Field f) { return (f & ESCAPED_BIT) != 0; }
static bool field_real(Field f) { return f != ABSENT && f != TOO_LONG; }

/* ------------------------------------------------------------------------- */
/* SWAR scanning: eight bytes per step, arithmetic rather than comparison.      */
/* Subtracting ones borrows across a byte only where that byte was zero, and    */
/* ~diff cancels the false positives the borrow creates.                        */
/* ------------------------------------------------------------------------- */

#define ONES UINT64_C(0x0101010101010101)
#define HIGH UINT64_C(0x8080808080808080)

static uint64_t broadcast(unsigned char b) { return (uint64_t)b * ONES; }

static uint64_t match_bits(uint64_t w, uint64_t target) {
    uint64_t diff = w ^ target;
    return (diff - ONES) & ~diff & HIGH;
}

static uint64_t load64(const char *p) {
    uint64_t w;
    memcpy(&w, p, sizeof w);
    return w; /* x86-64 and aarch64 are little-endian; this port targets those */
}

static size_t next_of2(const char *d, size_t from, size_t end, char a, char b) {
    uint64_t ba = broadcast((unsigned char)a), bb = broadcast((unsigned char)b);
    size_t at = from;
    for (; at + 8 <= end; at += 8) {
        uint64_t w = load64(d + at);
        uint64_t hits = match_bits(w, ba) | match_bits(w, bb);
        if (hits) return at + (size_t)(__builtin_ctzll(hits) >> 3);
    }
    for (; at < end; at++)
        if (d[at] == a || d[at] == b) return at;
    return end;
}

static size_t next_of1(const char *d, size_t from, size_t end, char t) {
    uint64_t bt = broadcast((unsigned char)t);
    size_t at = from;
    for (; at + 8 <= end; at += 8) {
        uint64_t w = load64(d + at);
        uint64_t hits = match_bits(w, bt);
        if (hits) return at + (size_t)(__builtin_ctzll(hits) >> 3);
    }
    for (; at < end; at++)
        if (d[at] == t) return at;
    return end;
}

static size_t skip_quoted(const char *d, size_t from, size_t end) {
    size_t at = from;
    for (;;) {
        size_t q = next_of1(d, at, end, '"');
        if (q >= end) return end;
        if (q + 1 < end && d[q + 1] == '"') { at = q + 2; continue; }
        return q + 1;
    }
}

/* ------------------------------------------------------------------------- */
/* The mapped file, and reading a field's logical bytes                         */
/* ------------------------------------------------------------------------- */

typedef struct {
    const char *data;
    size_t size;
    int fd;
} Slab;

static bool slab_open(Slab *s, const char *path) {
    s->fd = open(path, O_RDONLY);
    if (s->fd < 0) return false;
    struct stat st;
    if (fstat(s->fd, &st) != 0) { close(s->fd); return false; }
    s->size = (size_t)st.st_size;
    s->data = NULL;
    if (s->size > 0) {
        void *p = mmap(NULL, s->size, PROT_READ, MAP_PRIVATE, s->fd, 0);
        if (p == MAP_FAILED) { close(s->fd); return false; }
        madvise(p, s->size, MADV_SEQUENTIAL); /* read once, front to back */
        s->data = p;
    }
    return true;
}

static void slab_close(Slab *s) {
    if (s->data) munmap((void *)s->data, s->size);
    if (s->fd >= 0) close(s->fd);
}

/*
 * A field's logical length: the raw span with the second quote of each doubled
 * pair dropped. Equality, hashing and printing all read a field through the same
 * two helpers, so they cannot disagree about its value.
 */
static size_t logical_len(const Slab *s, Field f) {
    if (!field_real(f)) return 0;
    size_t len = field_len(f);
    if (!field_escaped(f)) return len;
    const char *p = s->data + field_off(f);
    size_t n = 0;
    for (size_t i = 0; i < len; i++) {
        n++;
        if (p[i] == '"' && i + 1 < len && p[i + 1] == '"') i++;
    }
    return n;
}

/* Copies the logical bytes into buf, returning the count written. */
static size_t logical_copy(const Slab *s, Field f, char *buf, size_t cap) {
    if (!field_real(f)) return 0;
    size_t len = field_len(f);
    const char *p = s->data + field_off(f);
    if (!field_escaped(f)) {
        size_t n = len < cap ? len : cap;
        memcpy(buf, p, n);
        return n;
    }
    size_t n = 0;
    for (size_t i = 0; i < len && n < cap; i++) {
        buf[n++] = p[i];
        if (p[i] == '"' && i + 1 < len && p[i + 1] == '"') i++;
    }
    return n;
}

static bool same_bytes(const Slab *a, Field x, const Slab *b, Field y) {
    bool ex = field_real(x) && field_escaped(x);
    bool ey = field_real(y) && field_escaped(y);
    if (!ex && !ey) {
        size_t lx = field_len(x), ly = field_len(y);
        if (!field_real(x)) lx = 0;
        if (!field_real(y)) ly = 0;
        return lx == ly && memcmp(a->data + field_off(x), b->data + field_off(y), lx) == 0;
    }
    if (logical_len(a, x) != logical_len(b, y)) return false;
    /* Rare: only a field holding a doubled quote reaches here. */
    size_t n = logical_len(a, x);
    char sx[4096], sy[4096];
    if (n > sizeof sx) return false; /* refused upstream by the length cap */
    logical_copy(a, x, sx, sizeof sx);
    logical_copy(b, y, sy, sizeof sy);
    return memcmp(sx, sy, n) == 0;
}

static bool is_absent(const Slab *s, Field f) {
    (void)s;
    return !field_real(f) || field_len(f) == 0;
}

/* FNV-1a over exactly the bytes equality compares, by the same route. */
static uint64_t hash_field(const Slab *s, Field f, uint64_t seed) {
    const uint64_t PRIME = UINT64_C(0x100000001b3);
    uint64_t h = seed;
    if (is_absent(s, f)) return (h ^ UINT64_C(0x9e3779b97f4a7c15)) * PRIME;
    size_t len = field_len(f);
    const char *p = s->data + field_off(f);
    uint64_t n = 0;
    if (!field_escaped(f)) {
        for (size_t i = 0; i < len; i++) { h = (h ^ (unsigned char)p[i]) * PRIME; n++; }
    } else {
        for (size_t i = 0; i < len; i++) {
            h = (h ^ (unsigned char)p[i]) * PRIME;
            n++;
            if (p[i] == '"' && i + 1 < len && p[i + 1] == '"') i++;
        }
    }
    return (h ^ n) * PRIME;
}

/* ------------------------------------------------------------------------- */
/* Parsing                                                                     */
/* ------------------------------------------------------------------------- */

typedef struct {
    char delimiter;
    int *source;   /* where each projected column sits in the file, or -1 */
    size_t width;
    int last_needed;
} RowParser;

static size_t end_of_row(const char *d, size_t pos, size_t end) {
    size_t at = pos;
    while (at < end) {
        size_t next = next_of2(d, at, end, '\n', '"');
        if (next >= end) return end;
        if (d[next] == '"') { at = skip_quoted(d, next + 1, end); continue; }
        return next;
    }
    return end;
}

/*
 * Parses one row into out, returning the offset of the next row. Once the last
 * needed column has been read the rest of the row is skipped to its newline: on
 * twenty columns keyed on the first two, most of a row is never delimited.
 */
static size_t parse_row(const RowParser *p, const char *d, size_t start, size_t end, Field *out) {
    for (size_t i = 0; i < p->width; i++) out[i] = ABSENT;
    size_t pos = start;
    int column = 0;

    while (pos <= end) {
        Field field;
        size_t next;
        if (pos < end && d[pos] == '"') {
            size_t close = skip_quoted(d, pos + 1, end);
            size_t body_end = close > pos + 1 ? close - 1 : pos + 1;
            next = next_of2(d, close, end, p->delimiter, '\n');
            field = pack(pos + 1, body_end - (pos + 1),
                         next_of1(d, pos + 1, body_end, '"') < body_end);
        } else {
            next = next_of2(d, pos, end, p->delimiter, '\n');
            size_t stop = next;
            if (stop > pos && d[stop - 1] == '\r') stop--; /* CRLF behaves like LF */
            field = pack(pos, stop - pos, false);
        }
        if (column <= p->last_needed)
            for (size_t i = 0; i < p->width; i++)
                if (p->source[i] == column) out[i] = field;
        column++;

        if (next >= end) return end;
        if (d[next] == '\n') return next + 1;
        pos = next + 1;
        if (column > p->last_needed) {
            size_t eol = end_of_row(d, pos, end);
            return eol >= end ? end : eol + 1;
        }
    }
    return end;
}

/* ------------------------------------------------------------------------- */
/* The index: open addressing over primitive arrays                            */
/* ------------------------------------------------------------------------- */

typedef struct {
    const Slab *slab;
    const RowParser *parser;
    size_t key_size;
    uint64_t *row_start;
    uint64_t *row_hash;
    size_t rows, rows_cap;
    int32_t *table;
    size_t mask;
    int32_t *first_row;
    uint32_t *occurrences;
    size_t keys, keys_cap;
    Field *probe;      /* re-used by every lookup, so a probe is not an allocation */
    int64_t dup_keys, dup_rows;
    bool failed;       /* a field too long for the packed length */
} RowIndex;

#define TABLE_EMPTY (-1)

static bool grow(void **p, size_t *cap, size_t need, size_t elem) {
    if (need <= *cap) return true;
    size_t next = *cap ? *cap * 2 : 4096;
    while (next < need) next *= 2;
    void *fresh = realloc(*p, next * elem);
    if (!fresh) return false;
    *p = fresh;
    *cap = next;
    return true;
}

static size_t slot_of(const RowIndex *ix, uint64_t hash) {
    /* The high bits of an FNV hash are the well-mixed ones; fold them down. */
    return (size_t)((hash ^ (hash >> 32)) & ix->mask);
}

static void index_fields(const RowIndex *ix, int32_t row, Field *out) {
    parse_row(ix->parser, ix->slab->data, (size_t)ix->row_start[row], ix->slab->size, out);
}

static bool index_rehash(RowIndex *ix) {
    size_t size = (ix->mask + 1) * 2;
    int32_t *table = malloc(size * sizeof *table);
    if (!table) return false;
    for (size_t i = 0; i < size; i++) table[i] = TABLE_EMPTY;
    free(ix->table);
    ix->table = table;
    ix->mask = size - 1;
    for (size_t key = 0; key < ix->keys; key++) {
        size_t slot = slot_of(ix, ix->row_hash[ix->first_row[key]]);
        while (ix->table[slot] != TABLE_EMPTY) slot = (slot + 1) & ix->mask;
        ix->table[slot] = (int32_t)key;
    }
    return true;
}

static bool index_add(RowIndex *ix, size_t start, const Field *fields) {
    if (!grow((void **)&ix->row_start, &ix->rows_cap, ix->rows + 1, sizeof *ix->row_start))
        return false;
    size_t hash_cap = ix->rows_cap;
    uint64_t *hashes = realloc(ix->row_hash, hash_cap * sizeof *ix->row_hash);
    if (!hashes) return false;
    ix->row_hash = hashes;

    int32_t row = (int32_t)ix->rows;
    uint64_t hash = UINT64_C(0xcbf29ce484222325);
    for (size_t i = 0; i < ix->key_size; i++) hash = hash_field(ix->slab, fields[i], hash);
    ix->row_start[ix->rows] = start;
    ix->row_hash[ix->rows] = hash;
    ix->rows++;

    size_t slot = slot_of(ix, hash);
    for (;;) {
        int32_t at = ix->table[slot];
        if (at == TABLE_EMPTY) {
            if (!grow((void **)&ix->first_row, &ix->keys_cap, ix->keys + 1, sizeof *ix->first_row))
                return false;
            uint32_t *occ = realloc(ix->occurrences, ix->keys_cap * sizeof *ix->occurrences);
            if (!occ) return false;
            ix->occurrences = occ;
            ix->table[slot] = (int32_t)ix->keys;
            ix->first_row[ix->keys] = row;
            ix->occurrences[ix->keys] = 1;
            ix->keys++;
            if (ix->keys * 2 > ix->mask + 1 && !index_rehash(ix)) return false;
            return true;
        }
        int32_t candidate = ix->first_row[at];
        if (ix->row_hash[candidate] == hash) {
            index_fields(ix, candidate, ix->probe);
            bool ok = true;
            for (size_t i = 0; i < ix->key_size && ok; i++) {
                bool xa = is_absent(ix->slab, ix->probe[i]), ya = is_absent(ix->slab, fields[i]);
                ok = (xa || ya) ? (xa && ya) : same_bytes(ix->slab, ix->probe[i], ix->slab, fields[i]);
            }
            if (ok) {
                if (++ix->occurrences[at] == 2) {
                    ix->dup_keys++;
                    ix->dup_rows++; /* the first occurrence counts once the key repeats */
                }
                ix->dup_rows++;
                return true;
            }
        }
        slot = (slot + 1) & ix->mask;
    }
}

static bool index_build(RowIndex *ix, const Slab *slab, const RowParser *parser, size_t from,
                        size_t key_size) {
    memset(ix, 0, sizeof *ix);
    ix->slab = slab;
    ix->parser = parser;
    ix->key_size = key_size;
    ix->mask = (1u << 12) - 1;
    ix->table = malloc((ix->mask + 1) * sizeof *ix->table);
    ix->probe = malloc(parser->width * sizeof *ix->probe);
    Field *fields = malloc(parser->width * sizeof *fields);
    if (!ix->table || !ix->probe || !fields) { free(fields); return false; }
    for (size_t i = 0; i <= ix->mask; i++) ix->table[i] = TABLE_EMPTY;

    const char *d = slab->data;
    size_t end = slab->size, pos = from;
    bool ok = true;
    while (pos < end) {
        if (d[pos] == '\n') { pos++; continue; }            /* an empty line is not a row */
        if (d[pos] == '\r' && pos + 1 < end && d[pos + 1] == '\n') { pos += 2; continue; }
        size_t next = parse_row(parser, d, pos, end, fields);
        for (size_t i = 0; i < parser->width; i++)
            if (fields[i] == TOO_LONG) ix->failed = true;
        if (ix->failed) { ok = false; break; }
        if (!index_add(ix, pos, fields)) { ok = false; break; }
        if (next <= pos) break; /* no progress: a malformed tail, not an endless loop */
        pos = next;
    }
    free(fields);
    return ok;
}

static int32_t index_lookup(const RowIndex *ix, const Slab *other, const Field *fields,
                            uint64_t hash) {
    size_t slot = slot_of(ix, hash);
    for (;;) {
        int32_t at = ix->table[slot];
        if (at == TABLE_EMPTY) return -1;
        int32_t candidate = ix->first_row[at];
        if (ix->row_hash[candidate] == hash) {
            index_fields(ix, candidate, ix->probe);
            bool ok = true;
            for (size_t i = 0; i < ix->key_size && ok; i++) {
                bool xa = is_absent(ix->slab, ix->probe[i]), ya = is_absent(other, fields[i]);
                ok = (xa || ya) ? (xa && ya) : same_bytes(ix->slab, ix->probe[i], other, fields[i]);
            }
            if (ok) return candidate;
        }
        slot = (slot + 1) & ix->mask;
    }
}

static void index_free(RowIndex *ix) {
    free(ix->row_start);
    free(ix->row_hash);
    free(ix->table);
    free(ix->first_row);
    free(ix->occurrences);
    free(ix->probe);
}

/* ------------------------------------------------------------------------- */
/* Columns and the command line                                                */
/* ------------------------------------------------------------------------- */

typedef struct {
    char **items;
    size_t len;
} Names;

static void names_free(Names *n) {
    for (size_t i = 0; i < n->len; i++) free(n->items[i]);
    free(n->items);
}

static bool names_push(Names *n, char *v) {
    char **fresh = realloc(n->items, (n->len + 1) * sizeof *n->items);
    if (!fresh) return false;
    n->items = fresh;
    n->items[n->len++] = v;
    return true;
}

static int name_index(const Names *n, const char *needle) {
    for (size_t i = 0; i < n->len; i++)
        if (strcmp(n->items[i], needle) == 0) return (int)i;
    return -1;
}

static Names split_commas(const char *s) {
    Names out = {0};
    const char *start = s;
    for (;;) {
        const char *comma = strchr(start, ',');
        size_t len = comma ? (size_t)(comma - start) : strlen(start);
        if (len > 0) {
            char *piece = malloc(len + 1);
            if (!piece) break;
            memcpy(piece, start, len);
            piece[len] = 0;
            if (!names_push(&out, piece)) { free(piece); break; }
        }
        if (!comma) break;
        start = comma + 1;
    }
    return out;
}

static char detect_delimiter(const char *line, size_t len) {
    const char candidates[] = {',', ';', '\t', '|'};
    char best = ',';
    long best_count = -1;
    for (size_t c = 0; c < sizeof candidates; c++) {
        long n = 0;
        for (size_t i = 0; i < len; i++)
            if (line[i] == candidates[c]) n++;
        if (n > best_count) { best = candidates[c]; best_count = n; }
    }
    return best;
}

/* The header row's names, and where the first data row starts. */
static bool read_header(const Slab *s, char delimiter, Names *out, size_t *start) {
    const char *d = s->data;
    if (s->size == 0) return false;
    size_t pos = 0;
    for (;;) {
        Field field;
        size_t next;
        if (pos < s->size && d[pos] == '"') {
            size_t close = skip_quoted(d, pos + 1, s->size);
            size_t body_end = close > pos + 1 ? close - 1 : pos + 1;
            next = next_of2(d, close, s->size, delimiter, '\n');
            field = pack(pos + 1, body_end - (pos + 1),
                         next_of1(d, pos + 1, body_end, '"') < body_end);
        } else {
            next = next_of2(d, pos, s->size, delimiter, '\n');
            size_t stop = next;
            if (stop > pos && d[stop - 1] == '\r') stop--;
            field = pack(pos, stop - pos, false);
        }
        size_t n = logical_len(s, field);
        char *name = malloc(n + 1);
        if (!name) return false;
        logical_copy(s, field, name, n);
        name[n] = 0;
        if (!names_push(out, name)) { free(name); return false; }

        if (next >= s->size) { *start = s->size; return true; }
        if (d[next] == '\n') { *start = next + 1; return true; }
        pos = next + 1;
    }
}

static int fail(const char *message) {
    fprintf(stderr, "error: %s\n", message);
    return 2;
}

int main(int argc, char **argv) {
    if (argc < 2 || strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0) {
        printf("csvdiff - composite-key CSV comparison, byte-level, in C\n\n"
               "usage:\n  csvdiff compare A B -k COLS [-i COLS] [--json PATH]\n\n"
               "exit codes: 0 identical, 1 differences found, 2 error\n");
        return argc < 2 ? 2 : 0;
    }
    if (strcmp(argv[1], "compare") != 0) return fail("unknown command");

    Names key = {0}, ignore = {0};
    const char *a_path = NULL, *b_path = NULL, *json_path = NULL;
    for (int i = 2; i < argc; i++) {
        const char *f = argv[i];
        if ((!strcmp(f, "-k") || !strcmp(f, "--key")) && i + 1 < argc) key = split_commas(argv[++i]);
        else if ((!strcmp(f, "-i") || !strcmp(f, "--ignore")) && i + 1 < argc) ignore = split_commas(argv[++i]);
        else if (!strcmp(f, "--json") && i + 1 < argc) json_path = argv[++i];
        else if ((!strcmp(f, "-o") || !strcmp(f, "--out") || !strcmp(f, "--engine")) && i + 1 < argc) i++;
        else if (f[0] == '-') { names_free(&key); names_free(&ignore); return fail("unknown option"); }
        else if (!a_path) a_path = f;
        else if (!b_path) b_path = f;
    }
    if (!a_path || !b_path) { names_free(&key); names_free(&ignore); return fail("compare needs two files"); }
    if (key.len == 0) { names_free(&key); names_free(&ignore); return fail("--key is required"); }

    int status = 2;
    Slab a = {0}, b = {0};
    a.fd = b.fd = -1;
    Names a_head = {0}, b_head = {0}, compared = {0};
    RowIndex ai = {0}, bi = {0};
    int *a_src = NULL, *b_src = NULL;
    Field *fa = NULL, *fb = NULL;
    int64_t *col_changed = NULL;
    int64_t *col_blanked = NULL;
    int64_t *col_filled = NULL;

    if (!slab_open(&a, a_path) || !slab_open(&b, b_path)) { fail("cannot read one of the files"); goto done; }

    size_t a_nl = next_of1(a.data, 0, a.size, '\n'), b_nl = next_of1(b.data, 0, b.size, '\n');
    char a_delim = detect_delimiter(a.data, a_nl), b_delim = detect_delimiter(b.data, b_nl);
    size_t a_start = 0, b_start = 0;
    if (!read_header(&a, a_delim, &a_head, &a_start) || !read_header(&b, b_delim, &b_head, &b_start)) {
        fail("file has no header row");
        goto done;
    }
    for (size_t i = 0; i < key.len; i++)
        if (name_index(&a_head, key.items[i]) < 0 || name_index(&b_head, key.items[i]) < 0) {
            fail("key column(s) missing from one of the files");
            goto done;
        }
    for (size_t i = 0; i < a_head.len; i++) {
        const char *c = a_head.items[i];
        if (name_index(&b_head, c) >= 0 && name_index(&key, c) < 0 && name_index(&ignore, c) < 0) {
            char *dup = strdup(c);
            if (!dup || !names_push(&compared, dup)) { free(dup); fail("out of memory"); goto done; }
        }
    }

    size_t key_size = key.len, nc = compared.len, width = key_size + nc;
    a_src = malloc(width * sizeof *a_src);
    b_src = malloc(width * sizeof *b_src);
    fa = malloc(width * sizeof *fa);
    fb = malloc(width * sizeof *fb);
    col_changed = calloc(nc ? nc : 1, sizeof *col_changed);
    col_blanked = calloc(nc ? nc : 1, sizeof *col_blanked);
    col_filled = calloc(nc ? nc : 1, sizeof *col_filled);
    if (!a_src || !b_src || !fa || !fb || !col_changed || !col_blanked || !col_filled) {
        fail("out of memory");
        goto done;
    }
    for (size_t i = 0; i < width; i++) {
        const char *n = i < key_size ? key.items[i] : compared.items[i - key_size];
        a_src[i] = name_index(&a_head, n);
        b_src[i] = name_index(&b_head, n);
    }

    RowParser ap = {a_delim, a_src, width, 0}, bp = {b_delim, b_src, width, 0};
    for (size_t i = 0; i < width; i++) {
        if (a_src[i] > ap.last_needed) ap.last_needed = a_src[i];
        if (b_src[i] > bp.last_needed) bp.last_needed = b_src[i];
    }

    if (!index_build(&ai, &a, &ap, a_start, key_size) || !index_build(&bi, &b, &bp, b_start, key_size)) {
        fail(ai.failed || bi.failed ? "a field is larger than this engine packs" : "out of memory");
        goto done;
    }

    int64_t matched = 0, changed = 0, added = 0, removed = 0;
    for (size_t k = 0; k < ai.keys; k++) {
        int32_t row = ai.first_row[k];
        index_fields(&ai, row, fa);
        uint64_t hash = UINT64_C(0xcbf29ce484222325);
        for (size_t i = 0; i < key_size; i++) hash = hash_field(&a, fa[i], hash);
        int32_t mate = index_lookup(&bi, &a, fa, hash);
        if (mate < 0) { removed++; continue; }
        matched++;
        index_fields(&bi, mate, fb);
        bool any = false;
        for (size_t i = 0; i < nc; i++) {
            Field x = fa[key_size + i], y = fb[key_size + i];
            bool xa = is_absent(&a, x), ya = is_absent(&b, y);
            bool differs = (xa || ya) ? (xa != ya) : !same_bytes(&a, x, &b, y);
            if (differs) {
                any = true;
                col_changed[i]++;
                if (ya) { col_blanked[i]++; }
                if (xa) { col_filled[i]++; }
            }
        }
        if (any) changed++;
    }
    for (size_t k = 0; k < bi.keys; k++) {
        int32_t row = bi.first_row[k];
        index_fields(&bi, row, fb);
        uint64_t hash = UINT64_C(0xcbf29ce484222325);
        for (size_t i = 0; i < key_size; i++) hash = hash_field(&b, fb[i], hash);
        if (index_lookup(&ai, &b, fb, hash) < 0) added++;
    }

    if (json_path) {
        FILE *out = fopen(json_path, "w");
        if (!out) { fail("cannot write the JSON summary"); goto done; }
        fprintf(out,
                "{\"counts\":{\"a_rows\":%zu,\"b_rows\":%zu,\"a_keys\":%zu,\"b_keys\":%zu,"
                "\"matched\":%lld,\"unchanged\":%lld,\"changed\":%lld,\"added\":%lld,"
                "\"removed\":%lld,\"a_dup_keys\":%lld,\"a_dup_rows\":%lld,"
                "\"b_dup_keys\":%lld,\"b_dup_rows\":%lld},\"columns\":[",
                ai.rows, bi.rows, ai.keys, bi.keys, (long long)matched,
                (long long)(matched - changed), (long long)changed, (long long)added,
                (long long)removed, (long long)ai.dup_keys, (long long)ai.dup_rows,
                (long long)bi.dup_keys, (long long)bi.dup_rows);
        for (size_t i = 0; i < nc; i++)
            fprintf(out,
                    "%s{\"name\":\"%s\",\"changed\":%lld,\"blanked\":%lld,\"filled\":%lld}",
                    i ? "," : "", compared.items[i], (long long)col_changed[i],
                    (long long)col_blanked[i], (long long)col_filled[i]);
        fprintf(out, "]}");
        fclose(out);
    }

    printf("A %zu rows | B %zu rows | matched %lld (changed %lld) | added %lld | removed %lld"
           " | dup keys A %lld B %lld | turbo\n",
           ai.rows, bi.rows, (long long)matched, (long long)changed, (long long)added,
           (long long)removed, (long long)ai.dup_keys, (long long)bi.dup_keys);
    status = (changed == 0 && added == 0 && removed == 0) ? 0 : 1;

done:
    index_free(&ai);
    index_free(&bi);
    free(a_src); free(b_src); free(fa); free(fb);
    free(col_changed); free(col_blanked); free(col_filled);
    names_free(&a_head); names_free(&b_head); names_free(&compared);
    names_free(&key); names_free(&ignore);
    slab_close(&a);
    slab_close(&b);
    return status;
}
