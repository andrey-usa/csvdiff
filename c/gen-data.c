/*
 * Writes the same deterministic pair of files as the other generators, byte for
 * byte, as CSV, newline-delimited JSON, or uncompressed Parquet.
 *
 * The recipe is identical because it has to be: a benchmark number from one
 * generator is only comparable with a number from another if the bytes agree.
 * Every row is a pure function of its index -- the drift buckets come from a
 * hash of the row number, and money is carried in integer cents so no
 * language's rounding rule can enter into it.
 *
 * This exists so the C port can make its own fixtures and its own benchmark
 * inputs. Before it, every check of the Parquet path needed the C++ generator
 * built first, which is a whole second toolchain standing between a change and
 * knowing whether it broke anything.
 *
 *   cd c && make gen-data
 *   ./gen-data --rows 10m --out-dir data --prefix 10m
 *   ./gen-data --rows 10m --out-dir data --format parquet
 */
#define _GNU_SOURCE

#include "pqwrite.h"

#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define COLUMNS 20

static char *const kNames[COLUMNS] = {
    "account_id", "txn_id", "posting_date", "value_date", "currency", "amount", "fee",
    "balance", "status", "channel", "region", "branch_code", "product_code", "counterparty",
    "quantity", "rate", "category", "risk_flag", "note", "updated_at"
};

static const char *const kStatus[] = { "posted", "pending", "settled", "reversed" };
static const char *const kChannel[] = { "branch", "online", "mobile", "atm", "wire" };
static const char *const kRegion[] = { "EMEA", "NA", "APAC", "LATAM" };
static const char *const kCurrency[] = { "USD", "EUR", "GBP", "JPY" };
static const char *const kCategory[] = { "retail", "corporate", "treasury", "cards", "loans" };

/* Drift buckets, against a 0..9999 hash bucket per row. */
enum { CHG_STATUS = 300, CHG_AMOUNT = 150, CHG_BALANCE = 150, CHG_VALUE_DATE = 30 };
#define REMOVED_MOD 1000
#define ADDED_RATIO 1000
#define DUP_MOD     10000

/* 240 dates from 2026-01-01. The other generators get these from a date
 * library; this one only needs the same strings out. */
static char g_days[240][11];

/* Column-name lengths, computed once. Writing a JSON row calls for all twenty
 * of them, so taking strlen each time is forty million calls on a two-million
 * row file. */
static size_t g_name_len[COLUMNS];

static void build_days(void) {
    for (int i = 0; i < COLUMNS; i++) g_name_len[i] = strlen(kNames[i]);
    static const int len[] = { 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31 };
    int month = 0, day = 1;
    for (int i = 0; i < 240; i++) {
        char *p = g_days[i];
        memcpy(p, "2026-", 5);
        p += 5;
        *p++ = (char)('0' + (month + 1) / 10);
        *p++ = (char)('0' + (month + 1) % 10);
        *p++ = '-';
        *p++ = (char)('0' + day / 10);
        *p++ = (char)('0' + day % 10);
        *p = '\0';
        if (++day > len[month]) { day = 1; month++; }
    }
}

/* splitmix-style mix, matching the other implementations bit for bit. */
static uint64_t mix(int64_t i, int64_t salt, int64_t seed) {
    uint64_t x = (uint64_t)(i * 31 + salt + seed);
    x = (x ^ (x >> 30)) * UINT64_C(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)) * UINT64_C(0x94D049BB133111EB);
    return x ^ (x >> 31);
}

static int mod_of(int64_t i, int64_t salt, int64_t seed, int m) {
    return (int)(mix(i, salt, seed) % (uint64_t)m);
}

static char *put(char *p, const char *s) {
    size_t n = strlen(s);
    memcpy(p, s, n);
    return p + n;
}

static char *put_int(char *p, int64_t v) {
    char tmp[24];
    int n = 0;
    if (v == 0) tmp[n++] = '0';
    bool neg = v < 0;
    uint64_t u = neg ? (uint64_t)(-v) : (uint64_t)v;
    while (u) { tmp[n++] = (char)('0' + u % 10); u /= 10; }
    if (neg) *p++ = '-';
    while (n) *p++ = tmp[--n];
    return p;
}

/* Left-pads to `width` with zeroes, which is what the other generators do with
 * a format string. */
static char *put_pad(char *p, int64_t v, int width) {
    char tmp[24];
    int n = 0;
    if (v == 0) tmp[n++] = '0';
    uint64_t u = (uint64_t)v;
    while (u) { tmp[n++] = (char)('0' + u % 10); u /= 10; }
    for (int i = n; i < width; i++) *p++ = '0';
    while (n) *p++ = tmp[--n];
    return p;
}

/* An amount held in cents, written as a two-decimal number. */
static char *put_money(char *p, int64_t cents) {
    if (cents < 0) { *p++ = '-'; cents = -cents; }
    p = put_int(p, cents / 100);
    *p++ = '.';
    return put_pad(p, cents % 100, 2);
}

/*
 * One row of one side, as its twenty fields rather than as a line of text.
 *
 * The CSV, JSON and Parquet writers all take these, so the three formats come
 * from one recipe and cannot drift. An empty field is a value the row does not
 * have: CSV has no other way to say it, and Parquet writes it as a null, which
 * is what every reader here treats an empty CSV field as anyway.
 */
typedef struct {
    char        buf[512];
    const char *f[COLUMNS];
    size_t      n[COLUMNS];
} Row;

static void fields(Row *out, int64_t i, bool b, int64_t seed) {
    const int bucket = mod_of(i, 0, seed, 10000);
    int64_t amount_cents = mod_of(i, 21, seed, 900000000) - 100000000;
    int64_t balance_cents = mod_of(i, 31, seed, 2000000000);
    const char *st = kStatus[mod_of(i, 11, seed, 4)];
    const char *value_date = g_days[mod_of(i, 41, seed, 240)];

    if (b) {
        if (bucket < CHG_STATUS) {
            st = kStatus[(mod_of(i, 11, seed, 4) + 1) % 4];
        } else if (bucket < CHG_STATUS + CHG_AMOUNT) {
            amount_cents += 1234;
        } else if (bucket < CHG_STATUS + CHG_AMOUNT + CHG_BALANCE) {
            balance_cents = (balance_cents * 101 + 50) / 100;   /* +1%, half up, in cents */
        }
        if (bucket < CHG_VALUE_DATE) value_date = "";
    }

    char *p = out->buf;
    int c = 0;
#define DONE(end) do { char *e_ = (end); out->f[c] = p; out->n[c] = (size_t)(e_ - p); c++; p = e_; } while (0)

    DONE(put_pad(put(p, "ACC-"), (i * 7919) % 250000, 8));
    DONE(put_pad(put(p, "TXN-"), i, 11));
    DONE(put(p, g_days[mod_of(i, 1, seed, 240)]));
    DONE(put(p, value_date));
    DONE(put(p, kCurrency[mod_of(i, 51, seed, 4)]));
    DONE(put_money(p, amount_cents));
    DONE(put_money(p, mod_of(i, 61, seed, 5000)));
    DONE(put_money(p, balance_cents));
    DONE(put(p, st));
    DONE(put(p, kChannel[mod_of(i, 71, seed, 5)]));
    DONE(put(p, kRegion[mod_of(i, 81, seed, 4)]));
    DONE(put_pad(put(p, "BR"), mod_of(i, 91, seed, 900) + 100, 4));
    DONE(put_pad(put(p, "P"), mod_of(i, 101, seed, 5000), 5));
    DONE(put_pad(put(p, "CP-"), mod_of(i, 111, seed, 90000), 6));
    DONE(put_int(p, mod_of(i, 121, seed, 500) + 1));
    DONE(put_pad(put(p, "0."), mod_of(i, 131, seed, 1200), 4));
    DONE(put(p, kCategory[mod_of(i, 141, seed, 5)]));
    *p = mod_of(i, 151, seed, 20) == 0 ? 'Y' : 'N';
    DONE(p + 1);
    {
        char *q = put(p, "batch ");
        q = put_int(q, i % 997 + 1);
        q = put(q, " line ");
        DONE(put_int(q, i % 53 + 1));
    }
    DONE(put(p, b ? "2026-09-01 02:15:00" : "2026-08-01 02:15:00"));
#undef DONE
}

/*
 * Which row indices each side holds.
 *
 * A has every row plus a few duplicates; B drops one row in a thousand, repeats
 * some of the first half, and gains a tail of rows A never had. That is what
 * puts added, removed and duplicate keys into every comparison.
 */
#define EACH_ROW(b, rows, EMIT)                                                     \
    do {                                                                            \
        const int64_t dup_extra_ = (rows) / DUP_MOD > 1 ? (rows) / DUP_MOD : 1;      \
        const int64_t added_ = (rows) / ADDED_RATIO > 1 ? (rows) / ADDED_RATIO : 1;  \
        for (int64_t i_ = 0; i_ < (rows); i_++) {                                   \
            if (!(b)) { EMIT(i_); continue; }                                       \
            if (i_ % REMOVED_MOD != 7) EMIT(i_);                                    \
            if (i_ % DUP_MOD == 3 && i_ < (rows) / 2) EMIT(i_);                     \
        }                                                                           \
        if (!(b)) { for (int64_t i_ = 0; i_ < dup_extra_; i_++) EMIT(i_); }          \
        else { for (int64_t i_ = (rows); i_ < (rows) + added_; i_++) EMIT(i_); }     \
    } while (0)

/* A JSON string body. None of the generated values need escaping, but a
 * generator that only happens to be correct for its own data is a trap for
 * whoever changes the recipe next. */
static char *put_json(char *p, const char *v, size_t n) {
    for (size_t i = 0; i < n; i++) {
        char c = v[i];
        switch (c) {
            case '"':  *p++ = '\\'; *p++ = '"'; break;
            case '\\': *p++ = '\\'; *p++ = '\\'; break;
            case '\n': *p++ = '\\'; *p++ = 'n'; break;
            case '\r': *p++ = '\\'; *p++ = 'r'; break;
            case '\t': *p++ = '\\'; *p++ = 't'; break;
            default:
                if ((unsigned char)c < 0x20) p += sprintf(p, "\\u%04x", c);
                else *p++ = c;
        }
    }
    return p;
}

/* ------------------------------------------------------------------------- */
/* The three writers                                                           */
/* ------------------------------------------------------------------------- */

/*
 * A megabyte of output at a time, written with one `fwrite` per buffer rather
 * than one per row. A row is a couple of hundred bytes, so the per-call cost --
 * the lock, the branch on the stream's state, the memcpy into libc's own buffer
 * -- was being paid two million times for work that is a memcpy either way.
 */
#define OUT_CAP (1 << 20)

typedef struct { FILE *fh; char *buf; size_t n; } Out;

static int out_flush(Out *o) {
    if (o->n && fwrite(o->buf, 1, o->n, o->fh) != o->n) return -1;
    o->n = 0;
    return 0;
}

/* Room for one more row. Rows are bounded well under 4 KB. */
static inline int out_room(Out *o) {
    return o->n + 4096 <= OUT_CAP ? 0 : out_flush(o);
}

static int write_csv(const char *path, bool b, int64_t rows, int64_t seed) {
    FILE *fh = fopen(path, "w");
    if (!fh) return -1;
    static char obuf[OUT_CAP];
    Out o = { fh, obuf, 0 };
    fputs("account_id,txn_id,posting_date,value_date,currency,amount,fee,balance,status,channel,"
          "region,branch_code,product_code,counterparty,quantity,rate,category,risk_flag,note,"
          "updated_at\n", fh);
    Row r;
#define EMIT_CSV(i)                                                        \
    do {                                                                   \
        if (out_room(&o) != 0) { fclose(fh); return -1; }                  \
        fields(&r, (i), b, seed);                                          \
        char *p = o.buf + o.n;                                             \
        for (int c = 0; c < COLUMNS; c++) {                                \
            if (c) *p++ = ',';                                             \
            memcpy(p, r.f[c], r.n[c]);                                     \
            p += r.n[c];                                                   \
        }                                                                  \
        *p++ = '\n';                                                       \
        o.n = (size_t)(p - o.buf);                                         \
    } while (0)
    EACH_ROW(b, rows, EMIT_CSV);
#undef EMIT_CSV
    if (out_flush(&o) != 0) { fclose(fh); return -1; }
    return fclose(fh) == 0 ? 0 : -1;
}

static int write_json(const char *path, bool b, int64_t rows, int64_t seed) {
    FILE *fh = fopen(path, "w");
    if (!fh) return -1;
    static char obuf[OUT_CAP];
    Out o = { fh, obuf, 0 };
    Row r;
#define EMIT_JSON(i)                                                       \
    do {                                                                   \
        if (out_room(&o) != 0) { fclose(fh); return -1; }                  \
        fields(&r, (i), b, seed);                                          \
        char *p = o.buf + o.n;                                             \
        *p++ = '{';                                                        \
        for (int c = 0; c < COLUMNS; c++) {                                \
            if (c) *p++ = ',';                                             \
            *p++ = '"';                                                    \
            memcpy(p, kNames[c], g_name_len[c]);                           \
            p += g_name_len[c];                                            \
            *p++ = '"'; *p++ = ':';                                        \
            if (r.n[c] == 0) { memcpy(p, "null", 4); p += 4; continue; }    \
            *p++ = '"';                                                    \
            p = put_json(p, r.f[c], r.n[c]);                               \
            *p++ = '"';                                                    \
        }                                                                  \
        *p++ = '}'; *p++ = '\n';                                           \
        o.n = (size_t)(p - o.buf);                                         \
    } while (0)
    EACH_ROW(b, rows, EMIT_JSON);
#undef EMIT_JSON
    if (out_flush(&o) != 0) { fclose(fh); return -1; }
    return fclose(fh) == 0 ? 0 : -1;
}

static int write_parquet(const char *path, bool b, int64_t rows, int64_t seed,
                         size_t group_rows, size_t dict_limit) {
    PqWriter *w = pqw_open(path, kNames, COLUMNS, group_rows, dict_limit);
    if (!w) return -1;
    Row r;
    PqValue cells[COLUMNS];
    int bad = 0;
#define EMIT_PQ(i)                                                         \
    do {                                                                   \
        if (bad) break;                                                    \
        fields(&r, (i), b, seed);                                          \
        for (int c = 0; c < COLUMNS; c++) {                                \
            cells[c].p = r.f[c];                                           \
            cells[c].n = r.n[c];                                           \
            cells[c].null = r.n[c] == 0;                                   \
        }                                                                  \
        if (pqw_row(w, cells) != 0) bad = 1;                               \
    } while (0)
    EACH_ROW(b, rows, EMIT_PQ);
#undef EMIT_PQ
    return pqw_close(w) != 0 || bad ? -1 : 0;
}

/* ------------------------------------------------------------------------- */
/* The command line                                                            */
/* ------------------------------------------------------------------------- */

/* "10m" and "20k" as well as a plain count, which is how every table in this
 * project names a row count. */
static int64_t parse_rows(const char *s) {
    char *end = NULL;
    long long v = strtoll(s, &end, 10);
    if (end && (*end == 'k' || *end == 'K')) v *= 1000;
    else if (end && (*end == 'm' || *end == 'M')) v *= 1000000;
    return (int64_t)v;
}

static int usage(void) {
    fprintf(stderr,
            "usage: gen-data --rows 10m --out-dir DIR [--prefix P]\n"
            "                [--format csv|json|parquet] [--seed N]\n"
            "                [--row-group-size N] [--dict-limit N]\n\n"
            "Writes DIR/P_a.EXT and DIR/P_b.EXT. Parquet is uncompressed: this port\n"
            "carries no codec, by the same choice its reader makes.\n\n"
            "--dict-limit is how many distinct values a column may have in one row\n"
            "group before it gives up on the dictionary; lowering it produces the\n"
            "mixed-encoding columns a real writer emits at scale.\n");
    return 2;
}

int main(int argc, char **argv) {
    int64_t rows = 0, seed = 7;   /* the same default every generator here uses */
    const char *out_dir = NULL, *prefix = NULL, *format = "csv";
    size_t group_rows = 122880, dict_limit = 8192;   /* the same defaults as the other generators */

    for (int i = 1; i < argc; i++) {
        const char *f = argv[i];
        if (!strcmp(f, "--rows") && i + 1 < argc) rows = parse_rows(argv[++i]);
        else if (!strcmp(f, "--out-dir") && i + 1 < argc) out_dir = argv[++i];
        else if (!strcmp(f, "--prefix") && i + 1 < argc) prefix = argv[++i];
        else if (!strcmp(f, "--format") && i + 1 < argc) format = argv[++i];
        else if (!strcmp(f, "--seed") && i + 1 < argc) seed = parse_rows(argv[++i]);
        else if (!strcmp(f, "--row-group-size") && i + 1 < argc) group_rows = (size_t)parse_rows(argv[++i]);
        else if (!strcmp(f, "--dict-limit") && i + 1 < argc) dict_limit = (size_t)parse_rows(argv[++i]);
        else if (!strcmp(f, "--threads") && i + 1 < argc) i++;   /* accepted and ignored */
        else if (!strcmp(f, "--compression") && i + 1 < argc) {
            const char *c = argv[++i];
            if (strcmp(c, "none") != 0) {
                fprintf(stderr, "error: this generator writes uncompressed parquet only\n");
                return 2;
            }
        } else return usage();
    }
    if (rows <= 0 || !out_dir) return usage();
    if (!prefix) prefix = "data";
    build_days();

    const char *ext;
    if (!strcmp(format, "csv")) ext = ".csv";
    else if (!strcmp(format, "json")) ext = ".ndjson";
    else if (!strcmp(format, "parquet")) ext = ".unc.parquet";
    else return usage();

    for (int side = 0; side < 2; side++) {
        char path[4096];
        snprintf(path, sizeof path, "%s/%s_%c%s", out_dir, prefix, side ? 'b' : 'a', ext);
        const bool b = side == 1;
        int ok;
        if (!strcmp(format, "csv")) ok = write_csv(path, b, rows, seed);
        else if (!strcmp(format, "json")) ok = write_json(path, b, rows, seed);
        else ok = write_parquet(path, b, rows, seed, group_rows, dict_limit);
        if (ok != 0) {
            fprintf(stderr, "error: cannot write %s: %s\n", path,
                    strcmp(format, "parquet") ? "write failed" : pqw_error());
            return 1;
        }
    }
    return 0;
}
