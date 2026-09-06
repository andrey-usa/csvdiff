#include "csvdiff.hpp"

#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <bit>
#include <charconv>
#include <chrono>
#include <cmath>
#include <cstring>
#include <stdexcept>

namespace csvdiff {
namespace {

// ---------------------------------------------------------------------------
// A field packed into one word: offset, length, and whether it needs unescaping.
//
// 40 bits of offset addresses a terabyte and 23 bits of length a field of eight
// megabytes, which is more than a CSV cell has any business being. An over-long
// field is reported rather than truncated through the mask.
// ---------------------------------------------------------------------------

using Field = std::uint64_t;

constexpr Field kAbsent = ~0ULL;
constexpr Field kTooLong = ~0ULL - 1;
constexpr std::uint64_t kOffsetMask = (1ULL << 40) - 1;
constexpr unsigned kLengthShift = 40;
constexpr std::uint64_t kLengthMask = (1ULL << 23) - 1;
constexpr std::uint64_t kEscaped = 1ULL << 63;
constexpr std::uint64_t kMaxFieldLen = kLengthMask;

Field pack(std::size_t offset, std::size_t len, bool escaped) {
    if (len > kMaxFieldLen) return kTooLong;
    return (offset & kOffsetMask) | ((std::uint64_t(len) & kLengthMask) << kLengthShift) |
           (escaped ? kEscaped : 0);
}

std::size_t offset_of(Field f) { return f & kOffsetMask; }
std::size_t len_of(Field f) { return (f >> kLengthShift) & kLengthMask; }
bool is_escaped(Field f) { return (f & kEscaped) != 0; }
bool is_real(Field f) { return f != kAbsent && f != kTooLong; }

// ---------------------------------------------------------------------------
// SWAR scanning: eight bytes per step, using arithmetic rather than comparison.
// ---------------------------------------------------------------------------

constexpr std::uint64_t kOnes = 0x0101010101010101ULL;
constexpr std::uint64_t kHigh = 0x8080808080808080ULL;

std::uint64_t broadcast(unsigned char b) { return std::uint64_t(b) * kOnes; }

// Sets the high bit of every byte of `word` equal to `target`. Subtracting ones
// borrows across a byte only where that byte was zero, and `~diff` cancels the
// false positives the borrow creates.
std::uint64_t match_bits(std::uint64_t word, std::uint64_t target) {
    const std::uint64_t diff = word ^ target;
    return (diff - kOnes) & ~diff & kHigh;
}

// std::byteswap is C++23; this keeps the port buildable on a C++20 compiler.
// Marked maybe_unused because the branch that calls it is dead on a
// little-endian target, which clang treats as an error under -Werror.
[[maybe_unused]] constexpr std::uint64_t bswap64(std::uint64_t v) {
    return ((v & 0x00000000000000FFULL) << 56) | ((v & 0x000000000000FF00ULL) << 40) |
           ((v & 0x0000000000FF0000ULL) << 24) | ((v & 0x00000000FF000000ULL) << 8) |
           ((v & 0x000000FF00000000ULL) >> 8) | ((v & 0x0000FF0000000000ULL) >> 24) |
           ((v & 0x00FF000000000000ULL) >> 40) | ((v & 0xFF00000000000000ULL) >> 56);
}

// Reads eight bytes as one little-endian word. The SWAR tricks below all assume
// the first byte of the file is the lowest byte of the word.
std::uint64_t load64(const char* p) {
    std::uint64_t w;
    std::memcpy(&w, p, sizeof w);
    if constexpr (std::endian::native == std::endian::big) w = bswap64(w);
    return w;
}

std::size_t next_of2(std::string_view d, std::size_t from, std::size_t end, char a, char b) {
    const std::uint64_t ba = broadcast(static_cast<unsigned char>(a));
    const std::uint64_t bb = broadcast(static_cast<unsigned char>(b));
    std::size_t at = from;
    for (; at + 8 <= end; at += 8) {
        const std::uint64_t w = load64(d.data() + at);
        const std::uint64_t hits = match_bits(w, ba) | match_bits(w, bb);
        if (hits) return at + (std::countr_zero(hits) >> 3);
    }
    for (; at < end; ++at)
        if (d[at] == a || d[at] == b) return at;
    return end;
}

std::size_t next_of1(std::string_view d, std::size_t from, std::size_t end, char target) {
    const std::uint64_t bt = broadcast(static_cast<unsigned char>(target));
    std::size_t at = from;
    for (; at + 8 <= end; at += 8) {
        const std::uint64_t w = load64(d.data() + at);
        const std::uint64_t hits = match_bits(w, bt);
        if (hits) return at + (std::countr_zero(hits) >> 3);
    }
    for (; at < end; ++at)
        if (d[at] == target) return at;
    return end;
}

// Walks past a quoted field's body. A doubled quote inside is content.
std::size_t skip_quoted(std::string_view d, std::size_t from, std::size_t end) {
    std::size_t at = from;
    for (;;) {
        const std::size_t q = next_of1(d, at, end, '"');
        if (q >= end) return end;
        if (q + 1 < end && d[q + 1] == '"') {
            at = q + 2;
            continue;
        }
        return q + 1;
    }
}

// ---------------------------------------------------------------------------
// The mapped file
// ---------------------------------------------------------------------------

class Slab {
  public:
    explicit Slab(const std::string& path) {
        fd_ = ::open(path.c_str(), O_RDONLY);
        if (fd_ < 0) throw Error("cannot read " + path);
        struct stat st{};
        if (::fstat(fd_, &st) != 0) {
            ::close(fd_);
            throw Error("cannot read " + path);
        }
        size_ = static_cast<std::size_t>(st.st_size);
        if (size_ > 0) {
            void* p = ::mmap(nullptr, size_, PROT_READ, MAP_PRIVATE, fd_, 0);
            if (p == MAP_FAILED) {
                ::close(fd_);
                throw Error("cannot map " + path);
            }
            data_ = static_cast<const char*>(p);
            // The whole file is read once, front to back, exactly once per pass.
            ::madvise(const_cast<void*>(p), size_, MADV_SEQUENTIAL);
        }
    }

    ~Slab() {
        if (data_) ::munmap(const_cast<char*>(data_), size_);
        if (fd_ >= 0) ::close(fd_);
    }

    Slab(const Slab&) = delete;
    Slab& operator=(const Slab&) = delete;

    std::string_view bytes() const { return {data_, size_}; }

    // The field's raw span, still holding any doubled quotes.
    std::string_view raw(Field f) const {
        if (!is_real(f)) return {};
        return {data_ + offset_of(f), len_of(f)};
    }

  private:
    int fd_ = -1;
    const char* data_ = nullptr;
    std::size_t size_ = 0;
};

// A field's logical bytes: the raw span with the second quote of each doubled
// pair dropped. Equality, hashing and decoding all read a field through this one
// route, so they cannot disagree about its value — the property whose absence
// produced two silently wrong answers in the Java port.
template <typename Fn>
void for_each_byte(const Slab& s, Field f, Fn&& fn) {
    const std::string_view raw = s.raw(f);
    if (!is_real(f) || !is_escaped(f)) {
        for (char c : raw) fn(static_cast<unsigned char>(c));
        return;
    }
    for (std::size_t i = 0; i < raw.size(); ++i) {
        const char c = raw[i];
        fn(static_cast<unsigned char>(c));
        if (c == '"' && i + 1 < raw.size() && raw[i + 1] == '"') ++i;
    }
}

std::string text_of(const Slab& s, Field f) {
    if (!is_real(f) || !is_escaped(f)) return std::string(s.raw(f));
    std::string out;
    out.reserve(s.raw(f).size());
    for_each_byte(s, f, [&](unsigned char b) { out.push_back(static_cast<char>(b)); });
    return out;
}

bool same_bytes(const Slab& a, Field x, const Slab& b, Field y) {
    const bool ex = is_real(x) && is_escaped(x);
    const bool ey = is_real(y) && is_escaped(y);
    if (!ex && !ey) return a.raw(x) == b.raw(y);
    std::string lx, ly;
    for_each_byte(a, x, [&](unsigned char c) { lx.push_back(static_cast<char>(c)); });
    for_each_byte(b, y, [&](unsigned char c) { ly.push_back(static_cast<char>(c)); });
    return lx == ly;
}


// ---------------------------------------------------------------------------
// Values: raw bytes on the fast path, normalised strings when asked
// ---------------------------------------------------------------------------

bool needs_normalising(const Options& o) {
    return o.trim || o.ignore_case || o.empty_is_null || o.tolerance > 0.0;
}

std::string_view trimmed(std::string_view s) {
    const auto space = [](char c) { return static_cast<unsigned char>(c) <= ' '; };
    while (!s.empty() && space(s.front())) s.remove_prefix(1);
    while (!s.empty() && space(s.back())) s.remove_suffix(1);
    return s;
}

// The field as the value a row-at-a-time engine would have built for it: empty
// is absent, then --trim, then --ignore-case, then --empty-is-null.
Val value_of(const Slab& s, Field f, const Options& o) {
    if (!is_real(f)) return std::nullopt;
    std::string text = text_of(s, f);
    if (text.empty()) return std::nullopt;
    if (o.trim) text = std::string(trimmed(text));
    if (o.ignore_case) {
        // Folding case outside ASCII needs a Unicode table this port does not
        // carry, and folding it partially is worse than not folding it: CAFE and
        // cafe with an acute would compare equal in the ports that do fold and
        // unequal here, and nothing in the output would say why. So the ASCII
        // path is taken where it is provably right, and anything else is
        // refused by name. See cpp/README.md.
        for (unsigned char c : text)
            if (c >= 0x80)
                throw Error(
                    "--ignore-case on a field outside ASCII needs Unicode case folding, which "
                    "this port does not carry; use another implementation for that data");
        std::transform(text.begin(), text.end(), text.begin(), [](unsigned char c) {
            return static_cast<char>(std::tolower(c));
        });
    }
    if (o.empty_is_null && text.empty()) return std::nullopt;
    return text;
}

bool is_absent(const Slab& s, Field f, const Options& o) {
    if (!is_real(f) || len_of(f) == 0) return true;
    if (!needs_normalising(o)) return false;
    return !value_of(s, f, o).has_value();
}

bool same(const Slab& a, Field x, const Slab& b, Field y, const Options& o) {
    const bool xa = is_absent(a, x, o), yb = is_absent(b, y, o);
    if (xa || yb) return xa && yb;
    if (!needs_normalising(o)) return same_bytes(a, x, b, y);
    return value_of(a, x, o) == value_of(b, y, o);
}

// Deliberately stricter than strtod: "inf" and "nan" are ordinary text in a CSV,
// and treating them as numbers would make two unequal strings compare equal.
std::optional<double> as_number(std::string_view s) {
    s = trimmed(s);
    if (s.empty()) return std::nullopt;
    std::string_view body = s;
    if (body.front() == '+' || body.front() == '-') body.remove_prefix(1);
    if (body.empty()) return std::nullopt;
    if (!(std::isdigit(static_cast<unsigned char>(body.front())) || body.front() == '.'))
        return std::nullopt;
    for (char c : body)
        if (!(std::isdigit(static_cast<unsigned char>(c)) || c == '.' || c == 'e' || c == 'E' ||
              c == '+' || c == '-'))
            return std::nullopt;
    double out = 0;
    const auto* first = s.data();
    const auto res = std::from_chars(first, first + s.size(), out);
    if (res.ec != std::errc() || res.ptr != first + s.size()) return std::nullopt;
    if (!std::isfinite(out)) return std::nullopt;
    return out;
}

// SQL's IS DISTINCT FROM, with the tolerance applied where both sides parse.
bool cell_differs(const Slab& a, Field x, const Slab& b, Field y, const Options& o) {
    const bool xa = is_absent(a, x, o), yb = is_absent(b, y, o);
    if (xa && yb) return false;
    if (o.tolerance > 0.0 && !xa && !yb) {
        const auto nx = as_number(text_of(a, x));
        const auto ny = as_number(text_of(b, y));
        if (nx && ny) return std::fabs(*nx - *ny) > o.tolerance;
    }
    return !same(a, x, b, y, o);
}

// FNV-1a over exactly the bytes equality compares, by the same route.
std::uint64_t hash_field(const Slab& s, Field f, const Options& o, std::uint64_t seed) {
    constexpr std::uint64_t kPrime = 0x100000001b3ULL;
    std::uint64_t h = seed;
    if (is_absent(s, f, o)) return (h ^ 0x9e3779b97f4a7c15ULL) * kPrime;
    std::uint64_t len = 0;
    if (needs_normalising(o)) {
        const std::string v = value_of(s, f, o).value_or(std::string());
        for (unsigned char b : v) {
            h = (h ^ b) * kPrime;
            ++len;
        }
    } else {
        for_each_byte(s, f, [&](unsigned char b) {
            h = (h ^ b) * kPrime;
            ++len;
        });
    }
    return (h ^ len) * kPrime;
}

std::uint64_t key_hash(const Slab& s, const Field* fields, std::size_t key_size, const Options& o) {
    std::uint64_t h = 0xcbf29ce484222325ULL;
    for (std::size_t i = 0; i < key_size; ++i) h = hash_field(s, fields[i], o, h);
    return h;
}

// Orders key values the way every other port writes its sections: ascending,
// absent values last.
int compare_keys(const std::vector<Val>& x, const std::vector<Val>& y, std::size_t key_size) {
    for (std::size_t i = 0; i < key_size; ++i) {
        const bool xn = !x[i].has_value(), yn = !y[i].has_value();
        if (xn && yn) continue;
        if (xn) return 1;
        if (yn) return -1;
        const int c = x[i]->compare(*y[i]);
        if (c != 0) return c;
    }
    return 0;
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

// Splits rows into fields, projecting straight to the columns asked for. Once
// the last needed column has been read the rest of the row is skipped to its
// newline without its fields ever being delimited: on twenty columns keyed on
// the first two, most of a row is never looked at.
class RowParser {
  public:
    RowParser(char delimiter, std::vector<int> source)
        : delimiter_(delimiter), source_(std::move(source)) {
        for (int c : source_) last_needed_ = std::max(last_needed_, c);
    }

    // Parses one row into `out`, returning the offset of the next row. A row
    // shorter than the header leaves the missing fields absent, which is a
    // difference to report rather than a file to refuse.
    std::size_t parse(std::string_view d, std::size_t start, std::size_t end, Field* out) const {
        std::fill(out, out + source_.size(), kAbsent);
        std::size_t pos = start;
        int column = 0;

        while (pos <= end) {
            Field field;
            std::size_t next;
            if (pos < end && d[pos] == '"') {
                const std::size_t close = skip_quoted(d, pos + 1, end);
                const std::size_t body_end = close > pos + 1 ? close - 1 : pos + 1;
                next = next_of2(d, close, end, delimiter_, '\n');
                const bool escaped = next_of1(d, pos + 1, body_end, '"') < body_end;
                field = pack(pos + 1, body_end - (pos + 1), escaped);
            } else {
                next = next_of2(d, pos, end, delimiter_, '\n');
                std::size_t stop = next;
                if (stop > pos && d[stop - 1] == '\r') --stop;  // CRLF behaves like LF
                field = pack(pos, stop - pos, false);
            }
            store(column, field, out);
            ++column;

            if (next >= end) return end;
            if (d[next] == '\n') return next + 1;
            pos = next + 1;
            if (column > last_needed_) {
                const std::size_t eol = end_of_row(d, pos, end);
                return eol >= end ? end : eol + 1;
            }
        }
        return end;
    }

    std::size_t width() const { return source_.size(); }

  private:
    void store(int column, Field f, Field* out) const {
        if (column > last_needed_) return;
        for (std::size_t i = 0; i < source_.size(); ++i)
            if (source_[i] == column) out[i] = f;
    }

    static std::size_t end_of_row(std::string_view d, std::size_t pos, std::size_t end) {
        std::size_t at = pos;
        while (at < end) {
            const std::size_t next = next_of2(d, at, end, '\n', '"');
            if (next >= end) return end;
            if (d[next] == '"') {
                at = skip_quoted(d, next + 1, end);
                continue;
            }
            return next;
        }
        return end;
    }

    char delimiter_;
    std::vector<int> source_;
    int last_needed_ = 0;
};

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

// Open addressing over one file's rows. Everything is a primitive array: row
// starts, key hashes, and a table of key numbers masked into a power-of-two slot
// count. The index stores where a row starts rather than its fields, because an
// offset is eight bytes where the fields would be twenty times that; re-parsing
// is cheap because the parser stops at the last needed column.
class RowIndex {
  public:
    RowIndex(const Slab& slab, const RowParser& parser, std::size_t from, std::size_t key_size,
             const Options& opt)
        : slab_(slab), parser_(parser), key_size_(key_size), opt_(opt) {
        table_.assign(1 << 12, kEmpty);
        mask_ = table_.size() - 1;
        scratch_.assign(parser.width(), kAbsent);

        const std::string_view d = slab.bytes();
        const std::size_t end = d.size();
        std::vector<Field> fields(parser.width());
        std::size_t pos = from;
        while (pos < end) {
            // A line with nothing on it is not a row.
            if (d[pos] == '\n') {
                ++pos;
                continue;
            }
            if (d[pos] == '\r' && pos + 1 < end && d[pos + 1] == '\n') {
                pos += 2;
                continue;
            }
            const std::size_t next = parser.parse(d, pos, end, fields.data());
            for (Field f : fields)
                if (f == kTooLong)
                    throw Error("a field larger than " + std::to_string(kMaxFieldLen) +
                                " bytes is more than this engine packs");
            add(pos, fields.data());
            if (next <= pos) break;  // no progress: a malformed tail, not an endless loop
            pos = next;
        }
    }

    void fields_of(int row, Field* out) const {
        parser_.parse(slab_.bytes(), row_start_[row], slab_.bytes().size(), out);
    }

    // The row carrying `fields`' key, or -1. `other` is the slab those fields
    // live in, which is the opposite file when this is a join probe.
    int lookup(const Slab& other, const Field* fields, std::uint64_t hash) const {
        std::size_t slot = slot_of(hash);
        Field* probe = scratch_.data();
        for (;;) {
            const int at = table_[slot];
            if (at == kEmpty) return -1;
            const int candidate = first_row_[at];
            if (row_hash_[candidate] == hash) {
                fields_of(candidate, probe);
                bool ok = true;
                for (std::size_t i = 0; i < key_size_ && ok; ++i)
                    ok = same(slab_, probe[i], other, fields[i], opt_);
                if (ok) return candidate;
            }
            slot = (slot + 1) & mask_;
        }
    }

    const std::vector<int>& first_rows() const { return first_row_; }
    const std::vector<std::uint32_t>& occurrences() const { return occurrences_; }
    std::int64_t rows() const { return rows_; }
    std::int64_t unique_keys() const { return static_cast<std::int64_t>(first_row_.size()); }
    std::int64_t dup_keys() const { return dup_keys_; }
    std::int64_t dup_rows() const { return dup_rows_; }

  private:
    static constexpr int kEmpty = -1;

    void add(std::size_t start, const Field* fields) {
        ++rows_;
        const int row = static_cast<int>(row_start_.size());
        const std::uint64_t hash = key_hash(slab_, fields, key_size_, opt_);
        row_start_.push_back(start);
        row_hash_.push_back(hash);

        std::size_t slot = slot_of(hash);
        Field* probe = scratch_.data();
        for (;;) {
            const int at = table_[slot];
            if (at == kEmpty) {
                table_[slot] = static_cast<int>(first_row_.size());
                first_row_.push_back(row);
                occurrences_.push_back(1);
                if (first_row_.size() * 2 > table_.size()) rehash();
                return;
            }
            const int candidate = first_row_[at];
            if (row_hash_[candidate] == hash) {
                fields_of(candidate, probe);
                bool ok = true;
                for (std::size_t i = 0; i < key_size_ && ok; ++i)
                    ok = same(slab_, probe[i], slab_, fields[i], opt_);
                if (ok) {
                    if (++occurrences_[at] == 2) {
                        ++dup_keys_;
                        ++dup_rows_;  // the first occurrence counts once the key repeats
                    }
                    ++dup_rows_;
                    return;
                }
            }
            slot = (slot + 1) & mask_;
        }
    }

    // The high bits of an FNV hash are the well-mixed ones; fold them down.
    std::size_t slot_of(std::uint64_t hash) const { return (hash ^ (hash >> 32)) & mask_; }

    void rehash() {
        table_.assign(table_.size() * 2, kEmpty);
        mask_ = table_.size() - 1;
        for (std::size_t key = 0; key < first_row_.size(); ++key) {
            std::size_t slot = slot_of(row_hash_[first_row_[key]]);
            while (table_[slot] != kEmpty) slot = (slot + 1) & mask_;
            table_[slot] = static_cast<int>(key);
        }
    }

    const Slab& slab_;
    const RowParser& parser_;
    std::size_t key_size_;
    const Options& opt_;
    std::vector<std::size_t> row_start_;
    std::vector<std::uint64_t> row_hash_;
    std::vector<int> table_;
    std::size_t mask_ = 0;
    std::vector<int> first_row_;
    std::vector<std::uint32_t> occurrences_;
    // Re-used by every probe. A lookup happens once per distinct key, so a
    // vector constructed here would be one heap allocation per row of the file.
    mutable std::vector<Field> scratch_;
    std::int64_t rows_ = 0, dup_keys_ = 0, dup_rows_ = 0;
};

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

// Guesses the delimiter from the header line, defaulting to a comma.
char detect_delimiter(std::string_view header) {
    char best = ',';
    long best_count = -1;
    for (char c : {',', ';', '\t', '|'}) {
        const long n = std::count(header.begin(), header.end(), c);
        if (n > best_count) {
            best = c;
            best_count = n;
        }
    }
    return best;
}

// The header row's names, and where the first data row starts.
std::pair<std::vector<std::string>, std::size_t> read_header(const Slab& s, char delimiter,
                                                             const std::string& path) {
    const std::string_view d = s.bytes();
    if (d.empty()) throw Error("file has no header row: " + path);
    std::vector<std::string> names;
    std::size_t pos = 0;
    for (;;) {
        Field field;
        std::size_t next;
        if (pos < d.size() && d[pos] == '"') {
            const std::size_t close = skip_quoted(d, pos + 1, d.size());
            const std::size_t body_end = close > pos + 1 ? close - 1 : pos + 1;
            next = next_of2(d, close, d.size(), delimiter, '\n');
            field = pack(pos + 1, body_end - (pos + 1),
                         next_of1(d, pos + 1, body_end, '"') < body_end);
        } else {
            next = next_of2(d, pos, d.size(), delimiter, '\n');
            std::size_t stop = next;
            if (stop > pos && d[stop - 1] == '\r') --stop;
            field = pack(pos, stop - pos, false);
        }
        names.push_back(text_of(s, field));
        if (next >= d.size()) return {names, d.size()};
        if (d[next] == '\n') return {names, next + 1};
        pos = next + 1;
    }
}

struct Resolved {
    std::vector<std::string> compared, only_in_a, only_in_b;
};

Resolved resolve(const std::vector<std::string>& a, const std::vector<std::string>& b,
                 const Options& opt) {
    const auto has = [](const std::vector<std::string>& v, const std::string& n) {
        return std::find(v.begin(), v.end(), n) != v.end();
    };
    for (const auto& k : opt.key)
        if (!has(a, k) || !has(b, k)) throw Error("key column(s) missing from one of the files: " + k);

    Resolved out;
    for (const auto& c : a)
        if (!has(b, c) && !has(opt.key, c)) out.only_in_a.push_back(c);
    for (const auto& c : b)
        if (!has(a, c) && !has(opt.key, c)) out.only_in_b.push_back(c);

    if (!opt.compare.empty()) {
        for (const auto& c : opt.compare) {
            if (!has(a, c) || !has(b, c))
                throw Error("compared column missing from one of the files: " + c);
            out.compared.push_back(c);
        }
        return out;
    }
    for (const auto& c : a)
        if (has(b, c) && !has(opt.key, c) && !has(opt.ignore, c)) out.compared.push_back(c);
    return out;
}

// ---------------------------------------------------------------------------
// The join
// ---------------------------------------------------------------------------

// A row list that stops growing at the report cap but keeps counting: the counts
// are always exact and the embedded rows always capped, so holding more than the
// cap would only ever serve an export this port does not write.
struct Capped {
    std::vector<std::pair<int, int>> held;  // row, and the row it matched
    std::size_t cap;
    std::int64_t total = 0;

    explicit Capped(std::size_t c) : cap(c) {}

    void push(int row, int mate) {
        ++total;
        if (held.size() <= cap) held.emplace_back(row, mate);  // one past, to detect truncation
    }

    bool truncated() const { return total > static_cast<std::int64_t>(cap); }
};

std::vector<Val> row_values(const Slab& s, const RowIndex& idx, int row, std::size_t width,
                            const Options& o) {
    std::vector<Field> fields(width);
    idx.fields_of(row, fields.data());
    std::vector<Val> out;
    out.reserve(width);
    for (Field f : fields) out.push_back(value_of(s, f, o));
    return out;
}

}  // namespace

Result compare(const std::string& a_path, const std::string& b_path, const Options& opt) {
    const auto started = std::chrono::steady_clock::now();
    if (opt.key.empty()) throw Error("at least one key column is required");

    Slab a(a_path), b(b_path);
    const char a_delim =
        opt.delimiter.value_or(detect_delimiter(a.bytes().substr(
            0, next_of1(a.bytes(), 0, a.bytes().size(), '\n'))));
    const char b_delim =
        opt.delimiter.value_or(detect_delimiter(b.bytes().substr(
            0, next_of1(b.bytes(), 0, b.bytes().size(), '\n'))));

    auto [a_header, a_start] = read_header(a, a_delim, a_path);
    auto [b_header, b_start] = read_header(b, b_delim, b_path);
    const Resolved resolved = resolve(a_header, b_header, opt);

    const std::size_t key_size = opt.key.size();
    const std::size_t nc = resolved.compared.size();
    const std::size_t width = key_size + nc;

    std::vector<std::string> wanted = opt.key;
    wanted.insert(wanted.end(), resolved.compared.begin(), resolved.compared.end());
    const auto positions = [&](const std::vector<std::string>& header) {
        std::vector<int> out;
        for (const auto& n : wanted) {
            const auto it = std::find(header.begin(), header.end(), n);
            out.push_back(it == header.end() ? -1 : static_cast<int>(it - header.begin()));
        }
        return out;
    };

    const RowParser ap(a_delim, positions(a_header));
    const RowParser bp(b_delim, positions(b_header));
    const RowIndex ai(a, ap, a_start, key_size, opt);
    const RowIndex bi(b, bp, b_start, key_size, opt);

    Result r;
    r.key = opt.key;
    r.compared = resolved.compared;
    r.only_in_a = resolved.only_in_a;
    r.only_in_b = resolved.only_in_b;
    r.a_cols = a_header.size();
    r.b_cols = b_header.size();
    r.columns.resize(nc);
    for (std::size_t i = 0; i < nc; ++i) r.columns[i].name = resolved.compared[i];

    Capped changed(opt.max_rows), added(opt.max_rows), removed(opt.max_rows);
    std::int64_t matched = 0;
    std::vector<Field> fa(width), fb(width);

    // A's distinct keys, in first-appearance order, so a run is reproducible.
    for (int row : ai.first_rows()) {
        ai.fields_of(row, fa.data());
        const int mate = bi.lookup(a, fa.data(), key_hash(a, fa.data(), key_size, opt));
        if (mate < 0) {
            removed.push(row, -1);
            continue;
        }
        ++matched;
        bi.fields_of(mate, fb.data());
        bool any = false;
        for (std::size_t i = 0; i < nc; ++i) {
            const Field x = fa[key_size + i], y = fb[key_size + i];
            if (cell_differs(a, x, b, y, opt)) {
                any = true;
                ++r.columns[i].changed;
                if (is_absent(b, y, opt)) ++r.columns[i].blanked;
                if (is_absent(a, x, opt)) ++r.columns[i].filled;
            }
        }
        if (any) changed.push(row, mate);
    }
    for (int row : bi.first_rows()) {
        bi.fields_of(row, fb.data());
        if (ai.lookup(b, fb.data(), key_hash(b, fb.data(), key_size, opt)) < 0)
            added.push(row, -1);
    }

    // Only now does anything become a string, and only for the rows kept.
    for (const auto& [row, mate] : removed.held) r.removed.push_back(row_values(a, ai, row, width, opt));
    for (const auto& [row, mate] : added.held) r.added.push_back(row_values(b, bi, row, width, opt));

    std::vector<std::pair<std::vector<Val>, std::vector<Val>>> pairs;
    pairs.reserve(changed.held.size());
    for (const auto& [row, mate] : changed.held)
        pairs.emplace_back(row_values(a, ai, row, width, opt), row_values(b, bi, mate, width, opt));

    const auto by_key = [&](const std::vector<Val>& x, const std::vector<Val>& y) {
        return compare_keys(x, y, key_size) < 0;
    };
    std::stable_sort(r.removed.begin(), r.removed.end(), by_key);
    std::stable_sort(r.added.begin(), r.added.end(), by_key);
    std::stable_sort(pairs.begin(), pairs.end(),
                     [&](const auto& p, const auto& q) { return by_key(p.first, q.first); });

    for (const auto& [ar, br] : pairs) {
        ChangedRow out;
        out.key.assign(ar.begin(), ar.begin() + static_cast<long>(key_size));
        for (std::size_t i = 0; i < nc; ++i) {
            const Val& x = ar[key_size + i];
            const Val& y = br[key_size + i];
            bool differs_here;
            if (!x && !y) {
                differs_here = false;
            } else if (opt.tolerance > 0.0 && x && y) {
                const auto nx = as_number(*x), ny = as_number(*y);
                differs_here = (nx && ny) ? std::fabs(*nx - *ny) > opt.tolerance : x != y;
            } else {
                differs_here = x != y;
            }
            if (differs_here) out.cells.push_back({i, x, y});
        }
        r.changed.push_back(std::move(out));
    }

    const auto dup_section = [&](const Slab& s, const RowIndex& idx, std::vector<DupRow>& out,
                                 bool& truncated) {
        std::vector<DupRow> all;
        const auto& firsts = idx.first_rows();
        const auto& counts = idx.occurrences();
        for (std::size_t i = 0; i < firsts.size(); ++i) {
            if (counts[i] < 2) continue;
            auto values = row_values(s, idx, firsts[i], width, opt);
            values.resize(key_size);
            all.push_back({std::move(values), static_cast<std::int64_t>(counts[i])});
        }
        std::stable_sort(all.begin(), all.end(), [&](const DupRow& x, const DupRow& y) {
            if (x.count != y.count) return x.count > y.count;
            return compare_keys(x.key, y.key, key_size) < 0;
        });
        truncated = all.size() > opt.max_rows;
        all.resize(std::min(all.size(), opt.max_rows));
        out = std::move(all);
    };
    dup_section(a, ai, r.dup_a, r.dup_a_truncated);
    dup_section(b, bi, r.dup_b, r.dup_b_truncated);

    r.counts = {ai.rows(),      bi.rows(),        ai.unique_keys(), bi.unique_keys(),
                matched,        matched - changed.total, changed.total,
                added.total,    removed.total,    ai.dup_keys(),    ai.dup_rows(),
                bi.dup_keys(),  bi.dup_rows()};
    r.changed_truncated = changed.truncated();
    r.added_truncated = added.truncated();
    r.removed_truncated = removed.truncated();
    r.changed.resize(std::min(r.changed.size(), opt.max_rows));
    r.added.resize(std::min(r.added.size(), opt.max_rows));
    r.removed.resize(std::min(r.removed.size(), opt.max_rows));

    const std::chrono::duration<double> elapsed = std::chrono::steady_clock::now() - started;
    r.seconds = std::round(elapsed.count() * 1000.0) / 1000.0;
    return r;
}

}  // namespace csvdiff
