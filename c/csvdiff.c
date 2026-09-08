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
 * It reads CSV, and -- through parquet.c and pqdiff.c -- uncompressed Parquet,
 * which is a different comparison rather than a different parser: a column
 * store is joined on its key columns and then diffed a column at a time, and
 * never becomes rows at all. Both paths produce the same counts on the same
 * data, which is what test.sh checks.
 *
 * Build:  make            (see Makefile; it is three files now, not one)
 * Usage:  csvdiff compare A B -k COLS [-i COLS] [--json PATH] [--threads N]
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

#include "parallel.h"
#include "parquet.h"
#include "pqdiff.h"

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

/*
 * Which of the two text shapes a file is.
 *
 * It decides two things that cannot be decided separately: how a row is found,
 * and how a value's bytes are read back. CSV doubles a quote to escape it; JSON
 * puts a backslash in front. A field is stored as an offset and a length either
 * way, so the dialect has to travel with the file rather than with the field.
 */
typedef enum { DIALECT_CSV = 0, DIALECT_JSON = 1 } Dialect;

typedef struct {
    const char *data;
    size_t size;
    int fd;
    Dialect dialect;
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

/* ------------------------------------------------------------------------- */
/* Newline-delimited JSON                                                      */
/*                                                                             */
/* One object per line. A value's bytes are contiguous in the file, so a JSON   */
/* field stays an offset and a length into the mapping exactly as a CSV field   */
/* does -- nothing here builds a string per cell either.                        */
/* ------------------------------------------------------------------------- */

static bool json_space(char c) { return c == ' ' || c == '\t' || c == '\r' || c == '\n'; }

/* Past the closing quote of the string starting at `at`. Sets *escaped when it
 * holds a backslash, which is what says the value has to be decoded. */
static size_t skip_json_string(const char *d, size_t at, size_t end, bool *escaped) {
    at++;                                        /* the opening quote */
    for (;;) {
        size_t stop = next_of2(d, at, end, '"', '\\');
        if (stop >= end) return end;
        if (d[stop] == '"') return stop + 1;
        *escaped = true;
        at = stop + 2;                           /* the backslash and what it escapes */
        if (at > end) return end;
    }
}

/* Past a nested object or array. Not a cell value; skipped, not guessed at. */
static size_t skip_json_nested(const char *d, size_t pos, size_t end) {
    int depth = 0;
    while (pos < end) {
        char c = d[pos];
        if (c == '"') {
            bool ignored = false;
            size_t next = skip_json_string(d, pos, end, &ignored);
            if (next <= pos) return end;
            pos = next;
            continue;
        }
        if (c == '{' || c == '[') depth++;
        if (c == '}' || c == ']') {
            depth--;
            pos++;
            if (depth <= 0) return pos;
            continue;
        }
        pos++;
    }
    return end;
}

/* Past the end of this object's line. Records are newline-delimited, so a
 * newline outside a string ends the row. */
static size_t end_of_json_row(const char *d, size_t pos, size_t end) {
    while (pos < end) {
        size_t stop = next_of2(d, pos, end, '\n', '"');
        if (stop >= end) return end;
        if (d[stop] == '\n') return stop + 1;
        bool ignored = false;
        size_t next = skip_json_string(d, stop, end, &ignored);
        if (next <= stop) return end;
        pos = next;
    }
    return end;
}

static size_t utf8_put(unsigned cp, char *buf, size_t cap, size_t at) {
    unsigned char tmp[4];
    size_t n;
    if (cp < 0x80) { tmp[0] = (unsigned char)cp; n = 1; }
    else if (cp < 0x800) {
        tmp[0] = (unsigned char)(0xC0 | (cp >> 6));
        tmp[1] = (unsigned char)(0x80 | (cp & 0x3F));
        n = 2;
    } else if (cp < 0x10000) {
        tmp[0] = (unsigned char)(0xE0 | (cp >> 12));
        tmp[1] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        tmp[2] = (unsigned char)(0x80 | (cp & 0x3F));
        n = 3;
    } else {
        tmp[0] = (unsigned char)(0xF0 | (cp >> 18));
        tmp[1] = (unsigned char)(0x80 | ((cp >> 12) & 0x3F));
        tmp[2] = (unsigned char)(0x80 | ((cp >> 6) & 0x3F));
        tmp[3] = (unsigned char)(0x80 | (cp & 0x3F));
        n = 4;
    }
    for (size_t i = 0; i < n; i++)
        if (buf && at + i < cap) buf[at + i] = (char)tmp[i];
    return n;
}

static int hex_digit(char h) {
    if (h >= '0' && h <= '9') return h - '0';
    if (h >= 'a' && h <= 'f') return h - 'a' + 10;
    if (h >= 'A' && h <= 'F') return h - 'A' + 10;
    return -1;
}

/*
 * Decodes a JSON string body, returning the decoded length and writing the
 * first `cap` bytes of it when `buf` is given. One function rather than a
 * length pass and a copy pass, so the two can never disagree about what a value
 * is.
 *
 * `\uXXXX` becomes UTF-8 so that a value written escaped and the same value
 * written literally compare equal -- which they must, because a JSON writer is
 * free to escape either way.
 */
static size_t json_unescape(const char *p, size_t len, char *buf, size_t cap) {
    size_t out = 0;
    for (size_t i = 0; i < len; i++) {
        if (p[i] != '\\' || i + 1 >= len) {
            if (buf && out < cap) buf[out] = p[i];
            out++;
            continue;
        }
        char e = p[++i];
        char plain = 0;
        switch (e) {
            case 'n': plain = '\n'; break;
            case 't': plain = '\t'; break;
            case 'r': plain = '\r'; break;
            case 'b': plain = '\b'; break;
            case 'f': plain = '\f'; break;
            case '"': plain = '"'; break;
            case '\\': plain = '\\'; break;
            case '/': plain = '/'; break;
            default: break;
        }
        if (plain) {
            if (buf && out < cap) buf[out] = plain;
            out++;
            continue;
        }
        if (e != 'u' || i + 4 >= len) {          /* not an escape we know: as written */
            if (buf && out < cap) buf[out] = '\\';
            out++;
            if (buf && out < cap) buf[out] = e;
            out++;
            continue;
        }
        unsigned cp = 0;
        bool ok = true;
        for (int k = 1; k <= 4 && ok; k++) {
            int dg = hex_digit(p[i + (size_t)k]);
            if (dg < 0) ok = false;
            else cp = cp * 16 + (unsigned)dg;
        }
        if (!ok) {                               /* not four hex digits: as written */
            if (buf && out < cap) buf[out] = '\\';
            out++;
            if (buf && out < cap) buf[out] = e;
            out++;
            continue;
        }
        i += 4;
        /* A surrogate pair is one code point written as two escapes. */
        if (cp >= 0xD800 && cp <= 0xDBFF && i + 6 < len && p[i + 1] == '\\' && p[i + 2] == 'u') {
            unsigned lo = 0;
            bool ok2 = true;
            for (int k = 3; k <= 6 && ok2; k++) {
                int dg = hex_digit(p[i + (size_t)k]);
                if (dg < 0) ok2 = false;
                else lo = lo * 16 + (unsigned)dg;
            }
            if (ok2 && lo >= 0xDC00 && lo <= 0xDFFF) {
                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                i += 6;
            }
        }
        out += utf8_put(cp, buf, cap, out);
    }
    return out;
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
    if (s->dialect == DIALECT_JSON) return json_unescape(p, len, NULL, 0);
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
    if (s->dialect == DIALECT_JSON) {
        size_t n = json_unescape(p, len, buf, cap);
        return n < cap ? n : cap;
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
    /* Rare: only an escaped field reaches here -- a doubled quote in CSV, a
     * backslash in JSON. */
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
    } else if (s->dialect == DIALECT_JSON) {
        /* Hashed over the decoded bytes, because that is what equality compares.
         * The buffer is the same one same_bytes uses, and a longer value is
         * refused upstream by the length cap. */
        char tmp[4096];
        size_t m = json_unescape(p, len, tmp, sizeof tmp);
        if (m > sizeof tmp) m = sizeof tmp;
        for (size_t i = 0; i < m; i++) { h = (h ^ (unsigned char)tmp[i]) * PRIME; n++; }
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
    int *source;   /* CSV: where each projected column sits in the file, or -1 */
    size_t width;
    int last_needed;
    Dialect dialect;
    /*
     * The key columns are the first `key_size` slots, and most of the parsing
     * this program does wants only those: the sweep hashes them, and a probe
     * that lands on a matching hash confirms them. `key_last` is the file
     * column the last of them sits at, so a key-only parse of a twenty-column
     * row delimits two fields and then scans once to the newline.
     */
    size_t key_size;
    int key_last;
    /*
     * CSV addresses a value by column number, and the projection has to get
     * from that number to the slot it fills. Scanning `source` for it cost
     * `width` comparisons per column and so `width * width` per row -- four
     * hundred at twenty columns, which is the same four hundred the JSON path
     * has a hash table to avoid. `col_first[c]` is the first slot fed by file
     * column c and `slot_next` chains the rest, so a column costs one load.
     */
    int32_t *col_first;
    int32_t *slot_next;
    /*
     * JSON addresses a value by key where CSV addresses it by column number, so
     * the wanted names live here. `want[i]` is the key whose value belongs in
     * slot i, or NULL for a column this file does not have. `slot` is a small
     * open-addressed table from name to slot, so a key in the file costs one
     * hash rather than a walk of twenty names -- which at twenty columns would
     * be four hundred comparisons a row.
     */
    char **want;
    int32_t *slot;
    size_t slot_mask;
} RowParser;

/*
 * Inverts `source` into the column-to-slot chains. One file column can in
 * principle feed several slots, so this is a chain rather than an array, and it
 * is built back to front so those slots are visited in slot order -- the order
 * the scan it replaces filled them in. This port's flags cannot produce that
 * case today (the compared set is the common columns minus the keys, so a
 * column is never both), which is precisely why the chain is here rather than
 * an assumption that it cannot happen.
 */
static bool parser_index_columns(RowParser *p) {
    const size_t n = p->last_needed >= 0 ? (size_t)p->last_needed + 1 : 1;
    p->col_first = malloc(n * sizeof *p->col_first);
    p->slot_next = malloc((p->width ? p->width : 1) * sizeof *p->slot_next);
    if (!p->col_first || !p->slot_next) return false;
    for (size_t i = 0; i < n; i++) p->col_first[i] = -1;
    for (size_t i = p->width; i-- > 0;) {
        p->slot_next[i] = -1;
        if (p->source[i] < 0 || p->source[i] > p->last_needed) continue;
        p->slot_next[i] = p->col_first[p->source[i]];
        p->col_first[p->source[i]] = (int32_t)i;
    }
    return true;
}

static uint64_t name_hash(const char *p, size_t n) {
    uint64_t h = UINT64_C(0xcbf29ce484222325);
    for (size_t i = 0; i < n; i++) h = (h ^ (unsigned char)p[i]) * UINT64_C(0x100000001b3);
    return h;
}

/* Builds the name-to-slot table. `want` is borrowed, not owned. */
static bool parser_index_names(RowParser *p, char **want, size_t width) {
    p->want = want;
    size_t n = 16;
    while (n < width * 4) n <<= 1;
    p->slot = malloc(n * sizeof *p->slot);
    if (!p->slot) return false;
    for (size_t i = 0; i < n; i++) p->slot[i] = -1;
    p->slot_mask = n - 1;
    for (size_t i = 0; i < width; i++) {
        if (!want[i]) continue;
        size_t at = name_hash(want[i], strlen(want[i])) & p->slot_mask;
        while (p->slot[at] >= 0) at = (at + 1) & p->slot_mask;
        p->slot[at] = (int32_t)i;
    }
    return true;
}

static int parser_slot_for(const RowParser *p, const char *key, size_t len) {
    size_t at = name_hash(key, len) & p->slot_mask;
    for (;;) {
        int32_t i = p->slot[at];
        if (i < 0) return -1;
        const char *w = p->want[i];
        if (w && strlen(w) == len && memcmp(w, key, len) == 0) return (int)i;
        at = (at + 1) & p->slot_mask;
    }
}

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
/*
 * Walks one JSON object, keeping the values of the keys wanted. One pass over
 * the object, one hash per key it holds -- not a search per wanted column.
 */
static size_t parse_json_row(const RowParser *p, const char *d, size_t start, size_t end,
                             Field *out, size_t slots) {
    for (size_t i = 0; i < slots; i++) out[i] = ABSENT;
    size_t pos = start;
    while (pos < end && json_space(d[pos])) pos++;
    if (pos >= end) return end;
    if (d[pos] != '{') return end_of_json_row(d, pos, end);   /* not an object: skip the line */
    pos++;

    for (;;) {
        while (pos < end && json_space(d[pos])) pos++;
        if (pos >= end) break;
        if (d[pos] == '}') { pos++; break; }
        if (d[pos] == ',') { pos++; continue; }
        if (d[pos] != '"') break;                             /* malformed: stop this object */

        bool key_escaped = false;
        size_t key_from = pos + 1;
        size_t key_end = skip_json_string(d, pos, end, &key_escaped);
        if (key_end > end || key_end < 2 || key_end - 1 < key_from) break;
        size_t key_len = key_end - 1 - key_from;
        pos = key_end;
        while (pos < end && json_space(d[pos])) pos++;
        if (pos >= end || d[pos] != ':') break;
        pos++;
        while (pos < end && json_space(d[pos])) pos++;
        if (pos >= end) break;

        size_t from, to;
        bool escaped = false, absent = false;
        if (d[pos] == '"') {
            from = pos + 1;
            size_t close = skip_json_string(d, pos, end, &escaped);
            to = close > pos + 1 ? close - 1 : pos + 1;
            pos = close;
        } else if (d[pos] == '{' || d[pos] == '[') {
            /* Nested: not a cell value, and left absent rather than guessed at. */
            from = to = pos;
            pos = skip_json_nested(d, pos, end);
            absent = true;
        } else {
            /* A number, true, false or null: to the next comma, brace or space. */
            from = pos;
            while (pos < end && d[pos] != ',' && d[pos] != '}' && !json_space(d[pos])) pos++;
            to = pos;
            absent = (to - from == 4 && memcmp(d + from, "null", 4) == 0);
        }
        if (!absent) {
            int slot = parser_slot_for(p, d + key_from, key_len);
            if (slot >= 0 && (size_t)slot < slots) out[slot] = pack(from, to - from, escaped);
        }
    }
    return end_of_json_row(d, pos, end);
}

static size_t parse_csv_row(const RowParser *p, const char *d, size_t start, size_t end,
                            Field *out, int last_needed, size_t slots) {
    for (size_t i = 0; i < slots; i++) out[i] = ABSENT;
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
        if (column <= last_needed)
            for (int32_t sl = p->col_first[column]; sl >= 0; sl = p->slot_next[sl])
                if ((size_t)sl < slots) out[sl] = field;
        column++;

        if (next >= end) return end;
        if (d[next] == '\n') return next + 1;
        pos = next + 1;
        if (column > last_needed) {
            size_t eol = end_of_row(d, pos, end);
            return eol >= end ? end : eol + 1;
        }
    }
    return end;
}

static size_t parse_row(const RowParser *p, const char *d, size_t start, size_t end, Field *out) {
    if (p->dialect == DIALECT_JSON) return parse_json_row(p, d, start, end, out, p->width);
    return parse_csv_row(p, d, start, end, out, p->last_needed, p->width);
}

/*
 * The key columns alone, into the first `key_size` slots. Everything that only
 * needs a key -- the sweep, and the probe that confirms a hash match -- goes
 * through this rather than parsing twenty columns to look at two.
 */
static size_t parse_keys(const RowParser *p, const char *d, size_t start, size_t end,
                         Field *out) {
    if (p->dialect == DIALECT_JSON) return parse_json_row(p, d, start, end, out, p->key_size);
    return parse_csv_row(p, d, start, end, out, p->key_last, p->key_size);
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
    Field *probe2;     /* the other side of a lazy equality check, same reason */
    int64_t dup_keys, dup_rows;
    bool failed;       /* a field too long for the packed length */
} RowIndex;

#define TABLE_EMPTY (-1)

static size_t slot_of(const RowIndex *ix, uint64_t hash) {
    /* The high bits of an FNV hash are the well-mixed ones; fold them down. */
    return (size_t)((hash ^ (hash >> 32)) & ix->mask);
}

static void index_fields(const RowIndex *ix, int32_t row, Field *out) {
    parse_row(ix->parser, ix->slab->data, (size_t)ix->row_start[row], ix->slab->size, out);
}

static void index_keys(const RowIndex *ix, int32_t row, Field *out) {
    parse_keys(ix->parser, ix->slab->data, (size_t)ix->row_start[row], ix->slab->size, out);
}

/* ------------------------------------------------------------------------- */
/* Finding the rows, on every core                                             */
/* ------------------------------------------------------------------------- */

/* How many rows ahead to start the load for. See pqdiff.c, where the same
 * constant was measured against the same kind of table. */
#define PREFETCH_AHEAD 24

/* Below this there is nothing worth dividing: the boundary work would cost more
 * than the parsing it splits. */
#define SPLIT_FROM (4u << 20)

/* One chunk's rows, in the order they appear in it. */
typedef struct {
    uint64_t *start;
    uint64_t *hash;
    size_t    n, cap;
    bool      failed;   /* a field too long for the packed length */
    bool      oom;
} Chunk;

/*
 * Where each chunk begins, as offsets of real row starts.
 *
 * Walking forward from a nominal split is the whole difficulty: a newline inside
 * a quoted field is not a row boundary, and a thread starting mid-file cannot
 * tell whether it is inside such a field. Parity settles it. Every `"` toggles
 * in-quote state -- including both halves of a doubled quote, which toggles
 * twice and so leaves the state alone, which is exactly right -- so the number
 * of quotes before a position says whether that position is inside a field.
 * Counting them is a scan for one byte, far cheaper than parsing.
 */
static unsigned chunk_bounds(const Slab *s, size_t from, unsigned threads, size_t *bounds) {
    const char *d = s->data;
    const size_t end = s->size;
    if (threads <= 1 || end - from < SPLIT_FROM) {
        bounds[0] = from;
        bounds[1] = end;
        return 1;
    }
    unsigned n = 0;
    bounds[n++] = from;
    for (unsigned i = 1; i < threads; i++) {
        const size_t nominal = from + (end - from) * i / threads;
        size_t quotes = 0;
        for (size_t at = from; at < nominal;) {
            const size_t q = next_of1(d, at, nominal, '"');
            if (q >= nominal) break;
            quotes++;
            at = q + 1;
        }
        bool in_quotes = (quotes & 1) != 0;
        size_t at = nominal;
        for (; at < end; at++) {
            if (d[at] == '"') in_quotes = !in_quotes;
            else if (d[at] == '\n' && !in_quotes) { at++; break; }
        }
        if (at > bounds[n - 1] && at < end) bounds[n++] = at;
    }
    bounds[n] = end;
    return n;
}

static bool chunk_push(Chunk *c, size_t start, uint64_t hash) {
    if (c->n == c->cap) {
        size_t next = c->cap ? c->cap * 2 : 8192;
        uint64_t *st = realloc(c->start, next * sizeof *st);
        if (!st) return false;
        c->start = st;
        uint64_t *hs = realloc(c->hash, next * sizeof *hs);
        if (!hs) return false;
        c->hash = hs;
        c->cap = next;
    }
    c->start[c->n] = start;
    c->hash[c->n] = hash;
    c->n++;
    return true;
}

typedef struct {
    const Slab      *slab;
    const RowParser *parser;
    size_t           key_size;
    const size_t    *bounds;
    Chunk           *chunk;
} SweepCtx;

/*
 * Parses and hashes every row that *starts* in this chunk, running past its end
 * to finish the last one. This is the work worth splitting: it reads the file
 * and writes only its own chunk, so any number of threads may be inside it.
 */
static void sweep_part(void *vctx, unsigned p) {
    SweepCtx *c = vctx;
    Chunk *out = &c->chunk[p];
    const char *d = c->slab->data;
    const size_t end = c->slab->size, stop = c->bounds[p + 1];
    Field *fields = malloc(c->parser->width * sizeof *fields);
    if (!fields) { out->oom = true; return; }

    size_t pos = c->bounds[p];
    while (pos < stop) {
        if (d[pos] == '\n') { pos++; continue; }            /* an empty line is not a row */
        if (d[pos] == '\r' && pos + 1 < end && d[pos + 1] == '\n') { pos += 2; continue; }
        const size_t next = parse_keys(c->parser, d, pos, end, fields);
        for (size_t i = 0; i < c->key_size; i++)
            if (fields[i] == TOO_LONG) out->failed = true;
        /*
         * A field can only be longer than the packed length if its row is, and
         * a row that long is a once-in-a-file event -- so the columns this
         * sweep no longer reads are still checked, exactly rather than
         * conservatively, by re-parsing just those rows in full.
         */
        if (!out->failed && next - pos > MAX_FIELD) {
            parse_row(c->parser, d, pos, end, fields);
            for (size_t i = 0; i < c->parser->width; i++)
                if (fields[i] == TOO_LONG) out->failed = true;
        }
        if (out->failed) break;
        uint64_t hash = UINT64_C(0xcbf29ce484222325);
        for (size_t i = 0; i < c->key_size; i++) hash = hash_field(c->slab, fields[i], hash);
        if (!chunk_push(out, pos, hash)) { out->oom = true; break; }
        if (next <= pos) break; /* no progress: a malformed tail, not an endless loop */
        pos = next;
    }
    free(fields);
}

/*
 * Rows are found and hashed on every core; they are inserted on one.
 *
 * The split is not arbitrary. A row's hash depends on nothing but that row, so
 * finding and hashing divides perfectly. Insertion does not: first-occurrence
 * wins, and which occurrence is first depends on the order rows arrive, so
 * threading it would make the answer depend on the scheduler.
 *
 * Two things the serial half does that the old single walk did not. The table
 * is sized once from the row count the sweep just established, where before it
 * started at 4,096 and doubled -- thirteen rehashes at ten million rows, each
 * one a full pass of random probes. And equality is checked lazily: a row's
 * fields are only re-parsed when a probe lands on an occupied slot whose hash
 * matches, which on ten million rows with a thousand duplicate keys is about a
 * thousand parses rather than ten million.
 */
static bool index_build(RowIndex *ix, const Slab *slab, const RowParser *parser, size_t from,
                        size_t key_size, unsigned threads) {
    memset(ix, 0, sizeof *ix);
    ix->slab = slab;
    ix->parser = parser;
    ix->key_size = key_size;
    ix->probe = malloc(parser->width * sizeof *ix->probe);
    ix->probe2 = malloc(parser->width * sizeof *ix->probe2);
    if (!ix->probe || !ix->probe2) return false;

    if (threads < 1) threads = 1;
    size_t *bounds = malloc((threads + 1) * sizeof *bounds);
    Chunk *chunks = calloc(threads, sizeof *chunks);
    if (!bounds || !chunks) { free(bounds); free(chunks); return false; }
    const unsigned ways = chunk_bounds(slab, from, threads, bounds);

    SweepCtx sc = { slab, parser, key_size, bounds, chunks };
    run_parts(sweep_part, &sc, ways);
    free(bounds);

    bool ok = true;
    for (unsigned p = 0; p < ways; p++) {
        if (chunks[p].failed) ix->failed = true;
        if (chunks[p].oom) ok = false;
        ix->rows += chunks[p].n;
    }
    if (ix->failed) ok = false;

    if (ok) {
        ix->row_start = malloc((ix->rows ? ix->rows : 1) * sizeof *ix->row_start);
        ix->row_hash = malloc((ix->rows ? ix->rows : 1) * sizeof *ix->row_hash);
        ok = ix->row_start && ix->row_hash;
    }
    if (ok) {
        /* Copied chunk by chunk, and each chunk released as it is taken, so the
         * high-water mark is the flat arrays plus one chunk rather than plus
         * all of them. */
        size_t at = 0;
        for (unsigned p = 0; p < ways; p++) {
            memcpy(ix->row_start + at, chunks[p].start, chunks[p].n * sizeof *ix->row_start);
            memcpy(ix->row_hash + at, chunks[p].hash, chunks[p].n * sizeof *ix->row_hash);
            at += chunks[p].n;
            free(chunks[p].start);
            free(chunks[p].hash);
            chunks[p].start = NULL;
            chunks[p].hash = NULL;
        }
        ix->rows_cap = ix->rows;
    }
    for (unsigned p = 0; p < ways; p++) { free(chunks[p].start); free(chunks[p].hash); }
    free(chunks);
    if (!ok) return false;

    /* Sized once, to under a half load, so nothing ever rehashes. */
    size_t cap = 1u << 12;
    while (cap < ix->rows * 2 + 16) cap <<= 1;
    ix->table = alloc_huge(cap * sizeof *ix->table);
    ix->first_row = malloc((ix->rows ? ix->rows : 1) * sizeof *ix->first_row);
    ix->occurrences = malloc((ix->rows ? ix->rows : 1) * sizeof *ix->occurrences);
    if (!ix->table || !ix->first_row || !ix->occurrences) return false;
    ix->keys_cap = ix->rows;
    ix->mask = cap - 1;
    memset(ix->table, 0xFF, cap * sizeof *ix->table);   /* TABLE_EMPTY is -1 */

    for (size_t r = 0; r < ix->rows; r++) {
        /* Every insert is a cache miss on a table too big to hold, and the hash
         * that decides which line is already in hand. */
        if (r + PREFETCH_AHEAD < ix->rows)
            __builtin_prefetch(&ix->table[slot_of(ix, ix->row_hash[r + PREFETCH_AHEAD])], 1, 0);
        const uint64_t hash = ix->row_hash[r];
        size_t slot = slot_of(ix, hash);
        for (;;) {
            const int32_t at = ix->table[slot];
            if (at == TABLE_EMPTY) {
                ix->table[slot] = (int32_t)ix->keys;
                ix->first_row[ix->keys] = (int32_t)r;
                ix->occurrences[ix->keys] = 1;
                ix->keys++;
                break;
            }
            const int32_t candidate = ix->first_row[at];
            if (ix->row_hash[candidate] == hash) {
                index_keys(ix, candidate, ix->probe);
                index_keys(ix, (int32_t)r, ix->probe2);
                bool same = true;
                for (size_t i = 0; i < key_size && same; i++) {
                    const bool xa = is_absent(slab, ix->probe[i]);
                    const bool ya = is_absent(slab, ix->probe2[i]);
                    same = (xa || ya) ? (xa && ya)
                                      : same_bytes(slab, ix->probe[i], slab, ix->probe2[i]);
                }
                if (same) {
                    if (++ix->occurrences[at] == 2) {
                        ix->dup_keys++;
                        ix->dup_rows++;  /* the first occurrence counts once the key repeats */
                    }
                    ix->dup_rows++;
                    break;
                }
            }
            slot = (slot + 1) & ix->mask;
        }
    }
    return true;
}

/* Both files' indexes, built at once. */
typedef struct {
    RowIndex        *ix[2];
    const Slab      *slab[2];
    const RowParser *parser[2];
    size_t           from[2];
    size_t           key_size;
    unsigned         threads;
    bool             ok[2];
} BuildCtx;

static void build_part(void *vctx, unsigned p) {
    BuildCtx *c = vctx;
    c->ok[p] = index_build(c->ix[p], c->slab[p], c->parser[p], c->from[p], c->key_size,
                           c->threads);
}

/*
 * `probe` is the caller's scratch, not the index's, because several threads are
 * inside this at once. It used to hang off the RowIndex, which was safe only
 * while the comparison ran on one thread.
 */
static int32_t index_lookup(const RowIndex *ix, const Slab *other, const Field *fields,
                            uint64_t hash, Field *probe) {
    size_t slot = slot_of(ix, hash);
    for (;;) {
        int32_t at = ix->table[slot];
        if (at == TABLE_EMPTY) return -1;
        int32_t candidate = ix->first_row[at];
        if (ix->row_hash[candidate] == hash) {
            index_keys(ix, candidate, probe);
            bool ok = true;
            for (size_t i = 0; i < ix->key_size && ok; i++) {
                bool xa = is_absent(ix->slab, probe[i]), ya = is_absent(other, fields[i]);
                ok = (xa || ya) ? (xa && ya) : same_bytes(ix->slab, probe[i], other, fields[i]);
            }
            if (ok) return candidate;
        }
        slot = (slot + 1) & ix->mask;
    }
}

/* ------------------------------------------------------------------------- */
/* Comparing, on every core                                                    */
/*                                                                             */
/* Every distinct key is independent of every other: it is looked up in the     */
/* other file's table and its columns compared, and nothing it does affects     */
/* what any other key finds. So the work splits over contiguous ranges of one   */
/* side's keys, each range counting into its own totals, and the totals are     */
/* summed at the end. Counts are sums, so the order they are added in cannot    */
/* change the answer.                                                          */
/* ------------------------------------------------------------------------- */

typedef struct {
    int64_t  matched, changed, removed, added;
    int64_t *col_changed, *col_blanked, *col_filled;
    Field   *fa, *fb, *probe;
    bool     oom;
} CmpPart;

typedef struct {
    const RowIndex *ai, *bi;
    const Slab     *a, *b;
    size_t          key_size, nc, width;
    unsigned        ways, b_ways;
    CmpPart        *parts;
} CmpCtx;

static void compare_part(void *vctx, unsigned p) {
    CmpCtx *c = vctx;
    CmpPart *out = &c->parts[p];
    const size_t key_size = c->key_size, nc = c->nc;

    out->fa = malloc(c->width * sizeof *out->fa);
    out->fb = malloc(c->width * sizeof *out->fb);
    out->probe = malloc(c->width * sizeof *out->probe);
    out->col_changed = calloc(nc ? nc : 1, sizeof *out->col_changed);
    out->col_blanked = calloc(nc ? nc : 1, sizeof *out->col_blanked);
    out->col_filled = calloc(nc ? nc : 1, sizeof *out->col_filled);
    if (!out->fa || !out->fb || !out->probe || !out->col_changed || !out->col_blanked ||
        !out->col_filled) {
        out->oom = true;
        return;
    }

    if (p < c->ways) {
        const size_t lo = c->ai->keys * p / c->ways, hi = c->ai->keys * (p + 1) / c->ways;
        for (size_t k = lo; k < hi; k++) {
            const int32_t row = c->ai->first_row[k];
            /* The sweep already hashed this row; re-deriving it here meant
             * hashing every key twice per run. */
            const uint64_t hash = c->ai->row_hash[row];
            if (k + PREFETCH_AHEAD < hi)
                __builtin_prefetch(
                    &c->bi->table[slot_of(c->bi,
                                          c->ai->row_hash[c->ai->first_row[k + PREFETCH_AHEAD]])],
                    0, 0);
            index_fields(c->ai, row, out->fa);
            const int32_t mate = index_lookup(c->bi, c->a, out->fa, hash, out->probe);
            if (mate < 0) { out->removed++; continue; }
            out->matched++;
            index_fields(c->bi, mate, out->fb);
            bool any = false;
            for (size_t i = 0; i < nc; i++) {
                const Field x = out->fa[key_size + i], y = out->fb[key_size + i];
                const bool xa = is_absent(c->a, x), ya = is_absent(c->b, y);
                const bool differs = (xa || ya) ? (xa != ya) : !same_bytes(c->a, x, c->b, y);
                if (differs) {
                    any = true;
                    out->col_changed[i]++;
                    if (ya) out->col_blanked[i]++;
                    if (xa) out->col_filled[i]++;
                }
            }
            if (any) out->changed++;
        }
        return;
    }

    const unsigned q = p - c->ways;
    const size_t lo = c->bi->keys * q / c->b_ways, hi = c->bi->keys * (q + 1) / c->b_ways;
    for (size_t k = lo; k < hi; k++) {
        const int32_t row = c->bi->first_row[k];
        const uint64_t hash = c->bi->row_hash[row];
        if (k + PREFETCH_AHEAD < hi)
            __builtin_prefetch(
                &c->ai->table[slot_of(c->ai,
                                      c->bi->row_hash[c->bi->first_row[k + PREFETCH_AHEAD]])],
                0, 0);
        index_keys(c->bi, row, out->fb);
        if (index_lookup(c->ai, c->b, out->fb, hash, out->probe) < 0) out->added++;
    }
}

static void index_free(RowIndex *ix) {
    free(ix->row_start);
    free(ix->row_hash);
    free(ix->table);
    free(ix->first_row);
    free(ix->occurrences);
    free(ix->probe);
    free(ix->probe2);
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
/*
 * Which shape the file is, from its first non-space byte. A JSON record starts
 * with `{`; nothing in a CSV header row can.
 */
static Dialect detect_dialect(const Slab *s) {
    for (size_t i = 0; i < s->size; i++) {
        if (json_space(s->data[i])) continue;
        return s->data[i] == '{' ? DIALECT_JSON : DIALECT_CSV;
    }
    return DIALECT_CSV;
}

/*
 * A JSON file has no header row, so the column names are the keys of its first
 * object. That is the same rule the C++ port uses, and the same limitation: a
 * key that appears only in later objects is not a column. It is the only rule
 * that costs one line to establish rather than a pass over the file.
 */
static bool json_header(const Slab *s, Names *out) {
    const char *d = s->data;
    size_t end = s->size, pos = 0;
    while (pos < end && json_space(d[pos])) pos++;
    if (pos >= end || d[pos] != '{') return false;
    pos++;
    for (;;) {
        while (pos < end && json_space(d[pos])) pos++;
        if (pos >= end || d[pos] == '}') break;
        if (d[pos] == ',') { pos++; continue; }
        if (d[pos] != '"') break;
        bool escaped = false;
        size_t from = pos + 1;
        size_t close = skip_json_string(d, pos, end, &escaped);
        if (close > end || close < 2 || close - 1 < from) break;
        size_t len = close - 1 - from;
        pos = close;
        while (pos < end && json_space(d[pos])) pos++;
        if (pos >= end || d[pos] != ':') break;
        pos++;
        while (pos < end && json_space(d[pos])) pos++;
        if (pos >= end) break;
        /* The key name is stored decoded, because that is how the parser will
         * see it and how --key spells it. */
        char *name = malloc(len + 1);
        if (!name) return false;
        size_t n = escaped ? json_unescape(d + from, len, name, len) : len;
        if (!escaped) memcpy(name, d + from, len);
        name[n <= len ? n : len] = '\0';
        if (!names_push(out, name)) { free(name); return false; }
        /* Step over the value. */
        if (d[pos] == '"') { bool ig = false; pos = skip_json_string(d, pos, end, &ig); }
        else if (d[pos] == '{' || d[pos] == '[') pos = skip_json_nested(d, pos, end);
        else while (pos < end && d[pos] != ',' && d[pos] != '}' && !json_space(d[pos])) pos++;
    }
    return out->len > 0;
}

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

/* ------------------------------------------------------------------------- */
/* The report                                                                  */
/*                                                                             */
/* Both paths end here, because the whole claim of the Parquet path is that it  */
/* is the same comparison on the same rows -- if it could print its own summary */
/* the two could drift apart without anything noticing.                         */
/* ------------------------------------------------------------------------- */

typedef struct {
    const char *name;
    long long   changed, blanked, filled;
} OutCol;

typedef struct {
    const char   *engine;
    long long     a_rows, b_rows, a_keys, b_keys;
    long long     matched, changed, added, removed;
    long long     a_dup_keys, a_dup_rows, b_dup_keys, b_dup_rows;
    const OutCol *cols;
    size_t        ncols;
} Summary;

/* Returns the process exit status: 0 identical, 1 differences, 2 error. */
static int emit(const Summary *s, const char *json_path) {
    if (json_path) {
        FILE *out = fopen(json_path, "w");
        if (!out) return fail("cannot write the JSON summary");
        fprintf(out,
                "{\"counts\":{\"a_rows\":%lld,\"b_rows\":%lld,\"a_keys\":%lld,\"b_keys\":%lld,"
                "\"matched\":%lld,\"unchanged\":%lld,\"changed\":%lld,\"added\":%lld,"
                "\"removed\":%lld,\"a_dup_keys\":%lld,\"a_dup_rows\":%lld,"
                "\"b_dup_keys\":%lld,\"b_dup_rows\":%lld},\"columns\":[",
                s->a_rows, s->b_rows, s->a_keys, s->b_keys, s->matched,
                s->matched - s->changed, s->changed, s->added, s->removed,
                s->a_dup_keys, s->a_dup_rows, s->b_dup_keys, s->b_dup_rows);
        for (size_t i = 0; i < s->ncols; i++)
            fprintf(out, "%s{\"name\":\"%s\",\"changed\":%lld,\"blanked\":%lld,\"filled\":%lld}",
                    i ? "," : "", s->cols[i].name, s->cols[i].changed, s->cols[i].blanked,
                    s->cols[i].filled);
        fprintf(out, "]}");
        fclose(out);
    }

    printf("A %lld rows | B %lld rows | matched %lld (changed %lld) | added %lld | removed %lld"
           " | dup keys A %lld B %lld | %s\n",
           s->a_rows, s->b_rows, s->matched, s->changed, s->added, s->removed,
           s->a_dup_keys, s->b_dup_keys, s->engine);
    return (s->changed == 0 && s->added == 0 && s->removed == 0) ? 0 : 1;
}

/* ------------------------------------------------------------------------- */
/* The Parquet path                                                            */
/* ------------------------------------------------------------------------- */

static int compare_parquet(const char *a_path, const char *b_path, const Names *key,
                           const Names *ignore, unsigned threads, const char *json_path) {
    PqResult r;
    if (pq_compare(a_path, b_path, key->items, key->len, ignore->items, ignore->len,
                   threads, &r) != 0)
        return fail(pq_error());

    OutCol *cols = calloc(r.ncols ? r.ncols : 1, sizeof *cols);
    if (!cols) { pq_result_free(&r); return fail("out of memory"); }
    for (size_t i = 0; i < r.ncols; i++) {
        cols[i].name = r.cols[i].name;
        cols[i].changed = r.cols[i].changed;
        cols[i].blanked = r.cols[i].blanked;
        cols[i].filled = r.cols[i].filled;
    }
    const Summary s = { "parquet", r.a_rows, r.b_rows, r.a_keys, r.b_keys, r.matched,
                        r.changed, r.added, r.removed, r.a_dup_keys, r.a_dup_rows,
                        r.b_dup_keys, r.b_dup_rows, cols, r.ncols };
    const int status = emit(&s, json_path);
    free(cols);
    pq_result_free(&r);
    return status;
}

int main(int argc, char **argv) {
    if (argc < 2 || strcmp(argv[1], "-h") == 0 || strcmp(argv[1], "--help") == 0) {
        printf("csvdiff - composite-key comparison, byte-level, in C\n\n"
               "usage:\n  csvdiff compare A B -k COLS [-i COLS] [--json PATH] [--threads N]\n\n"
               "CSV, newline-delimited JSON, and uncompressed Parquet when both files\n"
               "are Parquet. The dialect is detected from the bytes.\n"
               "exit codes: 0 identical, 1 differences found, 2 error\n");
        return argc < 2 ? 2 : 0;
    }
    if (strcmp(argv[1], "compare") != 0) return fail("unknown command");

    Names key = {0}, ignore = {0};
    const char *a_path = NULL, *b_path = NULL, *json_path = NULL;
    unsigned threads = 0;   /* 0 means one per core, on the Parquet path */
    for (int i = 2; i < argc; i++) {
        const char *f = argv[i];
        if ((!strcmp(f, "-k") || !strcmp(f, "--key")) && i + 1 < argc) key = split_commas(argv[++i]);
        else if ((!strcmp(f, "-i") || !strcmp(f, "--ignore")) && i + 1 < argc) ignore = split_commas(argv[++i]);
        else if (!strcmp(f, "--json") && i + 1 < argc) json_path = argv[++i];
        else if ((!strcmp(f, "-t") || !strcmp(f, "--threads")) && i + 1 < argc)
            threads = (unsigned)strtoul(argv[++i], NULL, 10);
        else if ((!strcmp(f, "-o") || !strcmp(f, "--out") || !strcmp(f, "--engine")) && i + 1 < argc) i++;
        else if (f[0] == '-') { names_free(&key); names_free(&ignore); return fail("unknown option"); }
        else if (!a_path) a_path = f;
        else if (!b_path) b_path = f;
    }
    if (!a_path || !b_path) { names_free(&key); names_free(&ignore); return fail("compare needs two files"); }
    if (key.len == 0) { names_free(&key); names_free(&ignore); return fail("--key is required"); }

    /* A column store and a byte stream have no common ground to be compared on:
     * one of them would have to be turned into the other, which is the cost the
     * columnar path exists to avoid. So a mixed pair is refused by name rather
     * than half-answered. */
    {
        const int ap = pq_is_parquet(a_path), bp = pq_is_parquet(b_path);
        if (ap != bp) {
            names_free(&key);
            names_free(&ignore);
            return fail("one file is parquet and the other is not; convert one of them first");
        }
        if (ap) {
            const int st = compare_parquet(a_path, b_path, &key, &ignore, threads, json_path);
            names_free(&key);
            names_free(&ignore);
            return st;
        }
    }

    int status = 2;
    Slab a = {0}, b = {0};
    a.fd = b.fd = -1;
    Names a_head = {0}, b_head = {0}, compared = {0};
    RowIndex ai = {0}, bi = {0};
    int *a_src = NULL, *b_src = NULL;
    char **want_a = NULL, **want_b = NULL;
    /* Declared here rather than where they are filled: the cleanup below frees
     * their name tables, and every `goto done` above that point would otherwise
     * be freeing an uninitialised pointer. */
    RowParser ap = {0}, bp = {0};
    Field *fa = NULL, *fb = NULL;
    int64_t *col_changed = NULL;
    int64_t *col_blanked = NULL;
    int64_t *col_filled = NULL;

    if (!slab_open(&a, a_path) || !slab_open(&b, b_path)) { fail("cannot read one of the files"); goto done; }

    a.dialect = detect_dialect(&a);
    b.dialect = detect_dialect(&b);
    size_t a_nl = next_of1(a.data, 0, a.size, '\n'), b_nl = next_of1(b.data, 0, b.size, '\n');
    char a_delim = detect_delimiter(a.data, a_nl), b_delim = detect_delimiter(b.data, b_nl);
    size_t a_start = 0, b_start = 0;
    /* A JSON file has no header row to skip, so its rows begin at byte zero. */
    bool a_ok = a.dialect == DIALECT_JSON ? json_header(&a, &a_head)
                                          : read_header(&a, a_delim, &a_head, &a_start);
    bool b_ok = b.dialect == DIALECT_JSON ? json_header(&b, &b_head)
                                          : read_header(&b, b_delim, &b_head, &b_start);
    if (!a_ok || !b_ok) {
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
    want_a = calloc(width, sizeof *want_a);
    want_b = calloc(width, sizeof *want_b);
    fa = malloc(width * sizeof *fa);
    fb = malloc(width * sizeof *fb);
    col_changed = calloc(nc ? nc : 1, sizeof *col_changed);
    col_blanked = calloc(nc ? nc : 1, sizeof *col_blanked);
    col_filled = calloc(nc ? nc : 1, sizeof *col_filled);
    if (!a_src || !b_src || !want_a || !want_b || !fa || !fb || !col_changed || !col_blanked ||
        !col_filled) {
        fail("out of memory");
        goto done;
    }
    for (size_t i = 0; i < width; i++) {
        const char *n = i < key_size ? key.items[i] : compared.items[i - key_size];
        a_src[i] = name_index(&a_head, n);
        b_src[i] = name_index(&b_head, n);
    }

    ap.delimiter = a_delim; ap.source = a_src; ap.width = width; ap.dialect = a.dialect;
    bp.delimiter = b_delim; bp.source = b_src; bp.width = width; bp.dialect = b.dialect;
    ap.key_size = bp.key_size = key_size;
    ap.last_needed = bp.last_needed = ap.key_last = bp.key_last = -1;
    for (size_t i = 0; i < width; i++) {
        if (a_src[i] > ap.last_needed) ap.last_needed = a_src[i];
        if (b_src[i] > bp.last_needed) bp.last_needed = b_src[i];
        if (i < key_size) {
            if (a_src[i] > ap.key_last) ap.key_last = a_src[i];
            if (b_src[i] > bp.key_last) bp.key_last = b_src[i];
        }
    }
    if (!parser_index_columns(&ap) || !parser_index_columns(&bp)) {
        fail("out of memory");
        goto done;
    }
    /* JSON looks a value up by name, so each parser is told which names it
     * wants and where each one belongs. A name the file does not have stays
     * NULL and its slot stays absent, which is how a column present on one side
     * only already behaves. */
    for (size_t i = 0; i < width; i++) {
        const char *n = i < key_size ? key.items[i] : compared.items[i - key_size];
        if (a_src[i] >= 0) want_a[i] = (char *)n;
        if (b_src[i] >= 0) want_b[i] = (char *)n;
    }
    if ((a.dialect == DIALECT_JSON && !parser_index_names(&ap, want_a, width)) ||
        (b.dialect == DIALECT_JSON && !parser_index_names(&bp, want_b, width))) {
        fail("out of memory");
        goto done;
    }

    {
        /*
         * Both files at once, each on half the budget. The sweep inside a build
         * is parallel but the insertion after it is not, so run one after the
         * other the machine sits half idle through both serial tails; overlapped,
         * one file's insertion runs against the other's sweep.
         */
        unsigned budget = threads ? threads : cpu_count();
        BuildCtx bc = { { &ai, &bi }, { &a, &b }, { &ap, &bp }, { a_start, b_start },
                        key_size, budget > 1 ? budget / 2 : 1, { false, false } };
        run_parts(build_part, &bc, 2);
        if (!bc.ok[0] || !bc.ok[1]) {
            fail(ai.failed || bi.failed ? "a field is larger than this engine packs"
                                        : "out of memory");
            goto done;
        }
    }

    int64_t matched = 0, changed = 0, added = 0, removed = 0;
    {
        unsigned budget = threads ? threads : cpu_count();
        unsigned ways = ai.keys < (1u << 14) ? 1u : budget;
        unsigned b_ways = bi.keys < (1u << 14) ? 1u : budget;
        CmpPart *parts = calloc(ways + b_ways, sizeof *parts);
        if (!parts) { fail("out of memory"); goto done; }
        CmpCtx cc = { &ai, &bi, &a, &b, key_size, nc, width, ways, b_ways, parts };
        run_parts(compare_part, &cc, ways + b_ways);

        bool oom = false;
        for (unsigned p = 0; p < ways + b_ways; p++) {
            const CmpPart *q = &parts[p];
            oom = oom || q->oom;
            matched += q->matched;
            changed += q->changed;
            removed += q->removed;
            added += q->added;
            for (size_t i = 0; i < nc && q->col_changed; i++) {
                col_changed[i] += q->col_changed[i];
                col_blanked[i] += q->col_blanked[i];
                col_filled[i] += q->col_filled[i];
            }
        }
        for (unsigned p = 0; p < ways + b_ways; p++) {
            free(parts[p].fa); free(parts[p].fb); free(parts[p].probe);
            free(parts[p].col_changed); free(parts[p].col_blanked); free(parts[p].col_filled);
        }
        free(parts);
        if (oom) { fail("out of memory"); goto done; }
    }

    {
        OutCol *out_cols = calloc(nc ? nc : 1, sizeof *out_cols);
        if (!out_cols) { fail("out of memory"); goto done; }
        for (size_t i = 0; i < nc; i++) {
            out_cols[i].name = compared.items[i];
            out_cols[i].changed = (long long)col_changed[i];
            out_cols[i].blanked = (long long)col_blanked[i];
            out_cols[i].filled = (long long)col_filled[i];
        }
        const Summary s = { "turbo", (long long)ai.rows, (long long)bi.rows,
                            (long long)ai.keys, (long long)bi.keys, matched, changed, added,
                            removed, (long long)ai.dup_keys, (long long)ai.dup_rows,
                            (long long)bi.dup_keys, (long long)bi.dup_rows, out_cols, nc };
        status = emit(&s, json_path);
        free(out_cols);
    }

done:
    index_free(&ai);
    index_free(&bi);
    free(a_src); free(b_src); free(want_a); free(want_b); free(fa); free(fb);
    free(ap.slot); free(bp.slot);
    free(ap.col_first); free(bp.col_first);
    free(ap.slot_next); free(bp.slot_next);
    free(col_changed); free(col_blanked); free(col_filled);
    names_free(&a_head); names_free(&b_head); names_free(&compared);
    names_free(&key); names_free(&ignore);
    slab_close(&a);
    slab_close(&b);
    return status;
}
