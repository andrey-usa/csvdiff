#include "csvdiff.hpp"
#include "pqdiff.hpp"

#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>

#include <algorithm>
#include <bit>
#include <optional>
#include <thread>
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

// Which text dialect a mapped file is in. It rides on the Slab because the
// escape rule has to follow the file: CSV doubles a quote, JSON puts a
// backslash in front of one, and `for_each_byte` is the single route every
// comparison reads a field through.
enum class Dialect { Csv, Json };

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

    Dialect dialect() const { return dialect_; }
    void set_dialect(Dialect d) { dialect_ = d; }

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
    Dialect dialect_ = Dialect::Csv;
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
    if (s.dialect() == Dialect::Csv) {
        for (std::size_t i = 0; i < raw.size(); ++i) {
            const char c = raw[i];
            fn(static_cast<unsigned char>(c));
            if (c == '"' && i + 1 < raw.size() && raw[i + 1] == '"') ++i;
        }
        return;
    }
    // JSON: a backslash escape. \uXXXX is decoded to UTF-8 so that a value
    // written escaped and the same value written literally compare equal --
    // which they must, because a JSON writer is free to escape either way.
    for (std::size_t i = 0; i < raw.size(); ++i) {
        if (raw[i] != '\\' || i + 1 >= raw.size()) {
            fn(static_cast<unsigned char>(raw[i]));
            continue;
        }
        const char e = raw[++i];
        switch (e) {
            case 'n': fn('\n'); break;
            case 't': fn('\t'); break;
            case 'r': fn('\r'); break;
            case 'b': fn('\b'); break;
            case 'f': fn('\f'); break;
            case '"': fn('"'); break;
            case '\\': fn('\\'); break;
            case '/': fn('/'); break;
            case 'u': {
                unsigned cp = 0;
                if (i + 4 < raw.size()) {
                    bool ok = true;
                    for (int k = 1; k <= 4 && ok; ++k) {
                        const char h = raw[i + static_cast<std::size_t>(k)];
                        const unsigned d = h >= '0' && h <= '9'   ? unsigned(h - '0')
                                           : h >= 'a' && h <= 'f' ? unsigned(h - 'a' + 10)
                                           : h >= 'A' && h <= 'F' ? unsigned(h - 'A' + 10)
                                                                  : 16u;
                        if (d == 16u) ok = false;
                        cp = cp * 16 + (ok ? d : 0);
                    }
                    if (!ok) {  // not four hex digits: emit it as written
                        fn('\\');
                        fn(static_cast<unsigned char>(e));
                        break;
                    }
                    i += 4;
                    // A surrogate pair is one code point in two escapes.
                    if (cp >= 0xD800 && cp <= 0xDBFF && i + 6 < raw.size() &&
                        raw[i + 1] == '\\' && raw[i + 2] == 'u') {
                        unsigned lo = 0;
                        bool ok2 = true;
                        for (int k = 3; k <= 6 && ok2; ++k) {
                            const char h = raw[i + static_cast<std::size_t>(k)];
                            const unsigned d = h >= '0' && h <= '9'   ? unsigned(h - '0')
                                               : h >= 'a' && h <= 'f' ? unsigned(h - 'a' + 10)
                                               : h >= 'A' && h <= 'F' ? unsigned(h - 'A' + 10)
                                                                      : 16u;
                            if (d == 16u) ok2 = false;
                            lo = lo * 16 + (ok2 ? d : 0);
                        }
                        if (ok2 && lo >= 0xDC00 && lo <= 0xDFFF) {
                            cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                            i += 6;
                        }
                    }
                    if (cp < 0x80) {
                        fn(static_cast<unsigned char>(cp));
                    } else if (cp < 0x800) {
                        fn(static_cast<unsigned char>(0xC0 | (cp >> 6)));
                        fn(static_cast<unsigned char>(0x80 | (cp & 0x3F)));
                    } else if (cp < 0x10000) {
                        fn(static_cast<unsigned char>(0xE0 | (cp >> 12)));
                        fn(static_cast<unsigned char>(0x80 | ((cp >> 6) & 0x3F)));
                        fn(static_cast<unsigned char>(0x80 | (cp & 0x3F)));
                    } else {
                        fn(static_cast<unsigned char>(0xF0 | (cp >> 18)));
                        fn(static_cast<unsigned char>(0x80 | ((cp >> 12) & 0x3F)));
                        fn(static_cast<unsigned char>(0x80 | ((cp >> 6) & 0x3F)));
                        fn(static_cast<unsigned char>(0x80 | (cp & 0x3F)));
                    }
                } else {
                    fn('\\');
                    fn(static_cast<unsigned char>(e));
                }
                break;
            }
            default:  // not an escape this dialect defines: emit both bytes
                fn('\\');
                fn(static_cast<unsigned char>(e));
                break;
        }
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
// One JSON object's worth of scanning: where a value starts and ends, and
// whether it carries a backslash. Values are contiguous bytes in the file, which
// is what lets a JSON field stay an offset and a length into the mapping rather
// than a string built per cell -- the same representation the CSV path uses.
struct JsonValue {
    std::size_t from = 0, to = 0;
    bool escaped = false;
    bool absent = false;  // JSON null, which compares as an empty cell
};

// Skips one JSON string starting at the opening quote, returning the offset one
// past the closing quote. `escaped` is set if it contains a backslash.
inline std::size_t skip_json_string(std::string_view d, std::size_t at, std::size_t end,
                                    bool* escaped) {
    ++at;  // the opening quote
    for (;;) {
        const std::size_t stop = next_of2(d, at, end, '"', '\\');
        if (stop >= end) return end;
        if (d[stop] == '"') return stop + 1;
        *escaped = true;
        at = stop + 2;  // the backslash and whatever it escapes
        if (at > end) return end;
    }
}

inline bool json_space(char c) { return c == ' ' || c == '\t' || c == '\r' || c == '\n'; }

class RowParser {
  public:
    RowParser(char delimiter, std::vector<int> source)
        : delimiter_(delimiter), source_(std::move(source)) {
        for (int c : source_) last_needed_ = std::max(last_needed_, c);
    }

    // The JSON form. A CSV row is addressed by column number; a JSON object is
    // addressed by key, so the wanted names are held here and looked up as the
    // object is walked. `wanted[i]` is the name whose value belongs in slot i;
    // an empty name is a column this file does not have.
    RowParser(Dialect dialect, std::vector<std::string> wanted)
        : dialect_(dialect), wanted_(std::move(wanted)) {
        // A small open-addressed table from key name to slot, so a key costs one
        // hash rather than a walk of twenty names. Sized to at least twice the
        // keys so probes stay short.
        std::size_t n = 16;
        while (n < wanted_.size() * 4) n <<= 1;
        slot_mask_ = n - 1;
        slots_.assign(n, -1);
        for (std::size_t i = 0; i < wanted_.size(); ++i) {
            if (wanted_[i].empty()) continue;
            std::size_t at = name_hash(wanted_[i]) & slot_mask_;
            while (slots_[at] >= 0) at = (at + 1) & slot_mask_;
            slots_[at] = static_cast<int>(i);
        }
    }

    // Parses one row into `out`, returning the offset of the next row. A row
    // shorter than the header leaves the missing fields absent, which is a
    // difference to report rather than a file to refuse.
    std::size_t parse(std::string_view d, std::size_t start, std::size_t end, Field* out) const {
        if (dialect_ == Dialect::Json) return parse_json(d, start, end, out);
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

    std::size_t width() const {
        return dialect_ == Dialect::Json ? wanted_.size() : source_.size();
    }

  private:
    static std::uint64_t name_hash(std::string_view s) {
        std::uint64_t h = 0xcbf29ce484222325ULL;
        for (char c : s) h = (h ^ static_cast<unsigned char>(c)) * 0x100000001b3ULL;
        return h ^ (h >> 32);
    }

    // Walks one JSON object, storing the values of the keys we want. One pass
    // over the object, one hash per key -- not a search per wanted column, which
    // at twenty columns would be four hundred comparisons a row.
    std::size_t parse_json(std::string_view d, std::size_t start, std::size_t end,
                           Field* out) const {
        std::fill(out, out + wanted_.size(), kAbsent);
        std::size_t pos = start;
        while (pos < end && json_space(d[pos])) ++pos;
        if (pos >= end) return end;
        if (d[pos] != '{') return end_of_json_row(d, pos, end);  // not an object: skip the line
        ++pos;

        for (;;) {
            while (pos < end && json_space(d[pos])) ++pos;
            if (pos >= end) break;
            if (d[pos] == '}') {
                ++pos;
                break;
            }
            if (d[pos] == ',') {
                ++pos;
                continue;
            }
            if (d[pos] != '"') break;  // malformed: stop reading this object
            bool key_escaped = false;
            const std::size_t key_from = pos + 1;
            const std::size_t key_end = skip_json_string(d, pos, end, &key_escaped);
            if (key_end > end || key_end < 2) break;
            const std::string_view key = d.substr(key_from, key_end - 1 - key_from);
            pos = key_end;
            while (pos < end && json_space(d[pos])) ++pos;
            if (pos >= end || d[pos] != ':') break;
            ++pos;
            while (pos < end && json_space(d[pos])) ++pos;
            if (pos >= end) break;

            JsonValue v;
            if (d[pos] == '"') {
                v.from = pos + 1;
                const std::size_t close = skip_json_string(d, pos, end, &v.escaped);
                v.to = close > pos + 1 ? close - 1 : pos + 1;
                pos = close;
            } else {
                // A number, true, false or null: runs to the next comma, brace
                // or space. Nested objects and arrays are not a cell value and
                // are left absent rather than guessed at.
                const std::size_t from = pos;
                if (d[pos] == '{' || d[pos] == '[') {
                    pos = skip_json_nested(d, pos, end);
                    v.absent = true;
                } else {
                    while (pos < end && d[pos] != ',' && d[pos] != '}' && !json_space(d[pos])) ++pos;
                    v.from = from;
                    v.to = pos;
                    v.absent = (pos - from == 4 && d.compare(from, 4, "null") == 0);
                }
            }
            if (!v.absent) {
                const int slot = slot_for(key);
                if (slot >= 0) out[slot] = pack(v.from, v.to - v.from, v.escaped);
            }
        }
        return end_of_json_row(d, pos, end);
    }

    int slot_for(std::string_view key) const {
        std::size_t at = name_hash(key) & slot_mask_;
        for (;;) {
            const int i = slots_[at];
            if (i < 0) return -1;
            if (wanted_[static_cast<std::size_t>(i)] == key) return i;
            at = (at + 1) & slot_mask_;
        }
    }

    // Past the end of this object's line. Records are newline-delimited, so a
    // newline outside a string ends the row.
    static std::size_t end_of_json_row(std::string_view d, std::size_t pos, std::size_t end) {
        while (pos < end) {
            const std::size_t stop = next_of2(d, pos, end, '\n', '"');
            if (stop >= end) return end;
            if (d[stop] == '\n') return stop + 1;
            bool ignored = false;
            pos = skip_json_string(d, stop, end, &ignored);
            if (pos <= stop) return end;
        }
        return end;
    }

  public:
    static std::size_t skip_nested_public(std::string_view d, std::size_t pos, std::size_t end) {
        return skip_json_nested(d, pos, end);
    }

  private:
    static std::size_t skip_json_nested(std::string_view d, std::size_t pos, std::size_t end) {
        int depth = 0;
        while (pos < end) {
            const char c = d[pos];
            if (c == '"') {
                bool ignored = false;
                pos = skip_json_string(d, pos, end, &ignored);
                continue;
            }
            if (c == '{' || c == '[') ++depth;
            if (c == '}' || c == ']') {
                --depth;
                ++pos;
                if (depth <= 0) return pos;
                continue;
            }
            ++pos;
        }
        return end;
    }

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

    char delimiter_ = ',';
    std::vector<int> source_;
    int last_needed_ = 0;
    Dialect dialect_ = Dialect::Csv;
    std::vector<std::string> wanted_;   // JSON: the key whose value goes in each slot
    std::vector<int> slots_;            // JSON: open-addressed name -> slot
    std::size_t slot_mask_ = 0;
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
    // Rows are found and hashed in `threads` parallel chunks, then inserted in
    // file order on this one. The split is safe because the two halves need
    // different things: parsing a row depends on nothing but where it starts,
    // while the table depends on the order rows arrive -- first occurrence of a
    // key wins, and duplicate counts follow from that. Doing the second half in
    // parallel would make the answer depend on thread scheduling.
    RowIndex(const Slab& slab, const RowParser& parser, std::size_t from, std::size_t key_size,
             const Options& opt, unsigned threads = 1)
        : slab_(slab), parser_(parser), key_size_(key_size), opt_(opt) {
        table_.assign(1 << 12, kEmpty);
        mask_ = table_.size() - 1;
        scratch_.assign(parser.width() * 2, kAbsent);  // probe, then this row's key

        const std::string_view d = slab.bytes();
        const std::size_t end = d.size();
        if (from >= end) return;

        const std::vector<std::size_t> bounds = chunk_bounds(d, from, threads);
        const std::size_t n = bounds.size() - 1;
        std::vector<Chunk> chunks(n);
        std::vector<std::exception_ptr> failures(n);

        auto scan = [&](std::size_t i) {
            try {
                sweep(d, parser, bounds[i], bounds[i + 1], end, chunks[i]);
            } catch (...) {
                failures[i] = std::current_exception();
            }
        };
        std::vector<std::thread> workers;
        workers.reserve(n - 1);
        for (std::size_t i = 1; i < n; ++i) {
            try {
                workers.emplace_back(scan, i);
            } catch (const std::system_error&) {
                scan(i);  // no thread to be had: the same work, here
            }
        }
        scan(0);
        for (auto& w : workers) w.join();
        for (const auto& f : failures)
            if (f) std::rethrow_exception(f);

        std::size_t total = 0;
        for (const auto& c : chunks) total += c.starts.size();
        row_start_.reserve(total);
        row_hash_.reserve(total);
        // Each chunk is released as soon as it has been inserted. Holding all of
        // them until the end would keep two copies of every row's start and hash
        // alive at once -- the chunks and the arrays being filled from them --
        // which is sixteen bytes a row of pure duplication, 320 MB at ten
        // million rows across both files. Freed as it goes, the two curves cross
        // instead of adding.
        for (auto& c : chunks) {
            for (std::size_t i = 0; i < c.starts.size(); ++i) insert(c.starts[i], c.hashes[i]);
            std::vector<std::size_t>().swap(c.starts);
            std::vector<std::uint64_t>().swap(c.hashes);
        }
    }

    void fields_of(int row, Field* out) const {
        parser_.parse(slab_.bytes(), row_start_[row], slab_.bytes().size(), out);
    }

    // The row carrying `fields`' key, or -1. `other` is the slab those fields
    // live in, which is the opposite file when this is a join probe. `probe` is
    // scratch the caller owns: the join runs both directions at once, and a
    // buffer hanging off the index would be shared between those threads.
    int lookup(const Slab& other, const Field* fields, std::uint64_t hash, Field* probe) const {
        std::size_t slot = slot_of(hash);
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

    // One chunk's rows, in the order they appear in it.
    struct Chunk {
        std::vector<std::size_t> starts;
        std::vector<std::uint64_t> hashes;
    };

    // Where each chunk begins, as offsets of real row starts. The nominal
    // split is `size / threads`, then walked forward to the next row.
    //
    // Walking forward is the whole difficulty: a newline inside a quoted field
    // is not a row boundary, and a thread starting mid-file cannot tell whether
    // it is inside such a field. Parity settles it. Every `"` toggles in-quote
    // state -- including both halves of a doubled quote, which toggles twice and
    // so leaves the state alone, which is exactly right -- so the count of
    // quotes before a position says whether that position is inside a field.
    // Counting them is a scan for one byte, far cheaper than parsing, and it
    // splits across the same threads.
    static std::vector<std::size_t> chunk_bounds(std::string_view d, std::size_t from,
                                                 unsigned threads) {
        const std::size_t end = d.size();
        // Below this there is nothing to divide: the boundary work would cost
        // more than the parsing it splits.
        if (threads <= 1 || end - from < (4u << 20)) return {from, end};

        std::vector<std::size_t> nominal;
        for (unsigned i = 1; i < threads; ++i)
            nominal.push_back(from + (end - from) * i / threads);

        // Quotes before each nominal split, counted in parallel.
        std::vector<std::size_t> quotes(nominal.size(), 0);
        {
            std::vector<std::thread> counters;
            auto count = [&](std::size_t i) {
                std::size_t n = 0;
                for (std::size_t at = from; at < nominal[i];) {
                    const std::size_t q = next_of1(d, at, nominal[i], '"');
                    if (q >= nominal[i]) break;
                    ++n;
                    at = q + 1;
                }
                quotes[i] = n;
            };
            counters.reserve(nominal.size() - 1);
            for (std::size_t i = 1; i < nominal.size(); ++i) {
                try {
                    counters.emplace_back(count, i);
                } catch (const std::system_error&) {
                    count(i);
                }
            }
            count(0);
            for (auto& c : counters) c.join();
        }

        std::vector<std::size_t> bounds{from};
        for (std::size_t i = 0; i < nominal.size(); ++i) {
            bool in_quotes = (quotes[i] & 1) != 0;
            std::size_t at = nominal[i];
            for (; at < end; ++at) {
                const char c = d[at];
                if (c == '"') {
                    in_quotes = !in_quotes;
                } else if (c == '\n' && !in_quotes) {
                    ++at;
                    break;
                }
            }
            if (at > bounds.back() && at < end) bounds.push_back(at);
        }
        bounds.push_back(end);
        return bounds;
    }

    // Parses and hashes every row that *starts* in [begin, stop), running past
    // `stop` to finish the last one. `end` is the end of the file. This is the
    // work worth splitting: it reads the members but writes only `out`, so any
    // number of threads may be inside it at once.
    void sweep(std::string_view d, const RowParser& parser, std::size_t begin, std::size_t stop,
               std::size_t end, Chunk& out) const {
        std::vector<Field> fields(parser.width());
        std::size_t pos = begin;
        while (pos < stop) {
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
            out.starts.push_back(pos);
            out.hashes.push_back(key_hash(slab_, fields.data(), key_size_, opt_));
            if (next <= pos) break;  // no progress: a malformed tail, not an endless loop
            pos = next;
        }
    }

    void insert(std::size_t start, std::uint64_t hash) {
        ++rows_;
        const int row = static_cast<int>(row_start_.size());
        row_start_.push_back(start);
        row_hash_.push_back(hash);

        std::size_t slot = slot_of(hash);
        // Two buffers, both only touched here, and only on the one thread that
        // runs the insertions: the candidate's key and this row's. This row's
        // fields are re-parsed rather than carried over from the sweep because
        // the sweep produced twenty million of them and this branch wants two.
        Field* probe = scratch_.data();
        Field* mine = scratch_.data() + parser_.width();
        bool mine_parsed = false;
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
                if (!mine_parsed) {
                    parser_.parse(slab_.bytes(), start, slab_.bytes().size(), mine);
                    mine_parsed = true;
                }
                fields_of(candidate, probe);
                bool ok = true;
                for (std::size_t i = 0; i < key_size_ && ok; ++i)
                    ok = same(slab_, probe[i], slab_, mine[i], opt_);
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

// Newline-delimited JSON if the first thing that is not whitespace is a brace.
// A CSV header can begin with anything else, and a `{` in the first column of a
// CSV header is not something this project has ever had to read.
Dialect sniff_dialect(std::string_view d) {
    for (std::size_t i = 0; i < d.size() && i < 64; ++i) {
        if (json_space(d[i])) continue;
        return d[i] == '{' ? Dialect::Json : Dialect::Csv;
    }
    return Dialect::Csv;
}

// A JSON file has no header row, so the column names are the keys of the first
// object, in the order it lists them. Two files may list them in different
// orders and still compare: the join is by name.
std::vector<std::string> json_header(const Slab& s, const std::string& path) {
    const std::string_view d = s.bytes();
    std::vector<std::string> names;
    std::size_t pos = 0;
    while (pos < d.size() && json_space(d[pos])) ++pos;
    if (pos >= d.size() || d[pos] != '{') throw Error("file has no JSON object to read: " + path);
    ++pos;
    for (;;) {
        while (pos < d.size() && json_space(d[pos])) ++pos;
        if (pos >= d.size() || d[pos] == '}') break;
        if (d[pos] == ',') {
            ++pos;
            continue;
        }
        if (d[pos] != '"') break;
        bool escaped = false;
        const std::size_t from = pos + 1;
        const std::size_t close = skip_json_string(d, pos, d.size(), &escaped);
        if (close <= from) break;
        names.emplace_back(d.substr(from, close - 1 - from));
        pos = close;
        while (pos < d.size() && json_space(d[pos])) ++pos;
        if (pos >= d.size() || d[pos] != ':') break;
        ++pos;
        while (pos < d.size() && json_space(d[pos])) ++pos;
        if (pos >= d.size()) break;
        if (d[pos] == '"') {
            bool ignore = false;
            pos = skip_json_string(d, pos, d.size(), &ignore);
        } else if (d[pos] == '{' || d[pos] == '[') {
            pos = RowParser::skip_nested_public(d, pos, d.size());
        } else {
            while (pos < d.size() && d[pos] != ',' && d[pos] != '}' && !json_space(d[pos])) ++pos;
        }
    }
    if (names.empty()) throw Error("the first JSON object has no keys: " + path);
    return names;
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

    // Parquet is not a text format and is not read as one: it goes to the
    // columnar path in pqdiff.cpp, which never materialises a row. Both sides
    // have to be Parquet -- comparing a column store against a byte stream
    // would mean building rows out of one of them, and that is the cost the
    // columnar path exists to avoid.
    {
        const bool ap = is_parquet(a_path), bp = is_parquet(b_path);
        if (ap != bp)
            throw Error("one file is parquet and the other is not; convert one of them first");
        if (ap) return compare_parquet(a_path, b_path, opt);
    }

    Slab a(a_path), b(b_path);
    // The two sides may be in different formats: comparing a CSV export against
    // the JSON the same pipeline emits is the case that motivates this.
    a.set_dialect(sniff_dialect(a.bytes()));
    b.set_dialect(sniff_dialect(b.bytes()));

    const char a_delim =
        opt.delimiter.value_or(detect_delimiter(a.bytes().substr(
            0, next_of1(a.bytes(), 0, a.bytes().size(), '\n'))));
    const char b_delim =
        opt.delimiter.value_or(detect_delimiter(b.bytes().substr(
            0, next_of1(b.bytes(), 0, b.bytes().size(), '\n'))));

    std::vector<std::string> a_header, b_header;
    std::size_t a_start = 0, b_start = 0;
    if (a.dialect() == Dialect::Json) {
        a_header = json_header(a, a_path);
    } else {
        std::tie(a_header, a_start) = read_header(a, a_delim, a_path);
    }
    if (b.dialect() == Dialect::Json) {
        b_header = json_header(b, b_path);
    } else {
        std::tie(b_header, b_start) = read_header(b, b_delim, b_path);
    }
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

    // A CSV parser is told which column number each slot comes from; a JSON one
    // is told which key. `wanted` is the same list either way.
    const auto names_for = [&](const std::vector<std::string>& header) {
        std::vector<std::string> out;
        for (const auto& n : wanted)
            out.push_back(std::find(header.begin(), header.end(), n) == header.end() ? "" : n);
        return out;
    };
    const RowParser ap = a.dialect() == Dialect::Json ? RowParser(Dialect::Json, names_for(a_header))
                                                     : RowParser(a_delim, positions(a_header));
    const RowParser bp = b.dialect() == Dialect::Json ? RowParser(Dialect::Json, names_for(b_header))
                                                     : RowParser(b_delim, positions(b_header));

    // The two indexes share nothing, so they are built at the same time, and
    // each is split further into chunks. Two files across N cores is N/2 chunks
    // each -- so the whole machine is busy, not just two of it. An exception
    // thrown on the worker is carried back and rethrown here, because a parse
    // error has to reach the caller as an error and not as a crash.
    unsigned budget = opt.threads;
    if (budget == 0) budget = std::max(1u, std::thread::hardware_concurrency());
    const unsigned per_file = std::max(1u, budget / 2);

    std::optional<RowIndex> ai_slot, bi_slot;
    std::exception_ptr worker_failure;
    {
        std::thread worker([&] {
            try {
                bi_slot.emplace(b, bp, b_start, key_size, opt, per_file);
            } catch (...) {
                worker_failure = std::current_exception();
            }
        });
        try {
            ai_slot.emplace(a, ap, a_start, key_size, opt, per_file);
        } catch (...) {
            worker.join();
            throw;
        }
        worker.join();
    }
    if (worker_failure) std::rethrow_exception(worker_failure);
    const RowIndex& ai = *ai_slot;
    const RowIndex& bi = *bi_slot;

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

    // The two directions of the join touch different outputs -- one fills
    // changed, removed and the column counts, the other only added -- and read
    // both indexes without writing either, so they run at the same time. Each
    // owns its field buffers and its probe scratch; nothing is shared but the
    // two finished indexes and the two mappings, which are const from here on.
    // A's side is the long pole: every distinct key is looked up in B, both rows
    // are re-parsed, and every compared column is examined. It splits over
    // contiguous ranges of A's keys. Each range accumulates into its own counts,
    // its own column stats and its own capped row lists; because the ranges are
    // contiguous and merged in order, the result is identical to one thread's,
    // including which rows survive the cap.
    struct Part {
        std::int64_t matched = 0;
        std::vector<ColumnStat> columns;
        std::vector<std::pair<int, int>> changed, removed;
        std::int64_t changed_total = 0, removed_total = 0;
    };

    const std::vector<int>& a_keys = ai.first_rows();
    unsigned join_ways = std::max(1u, budget > 1 ? budget - 1 : 1u);
    if (a_keys.size() < 1u << 14) join_ways = 1;  // too few keys to be worth splitting
    std::vector<Part> parts(join_ways);
    for (auto& part : parts) part.columns.resize(nc);

    auto a_range = [&](unsigned p) {
        Part& out = parts[p];
        const std::size_t lo = a_keys.size() * p / join_ways;
        const std::size_t hi = a_keys.size() * (p + 1) / join_ways;
        std::vector<Field> fa(width), fb(width), probe(width);
        for (std::size_t at = lo; at < hi; ++at) {
            const int row = a_keys[at];
            ai.fields_of(row, fa.data());
            const int mate =
                bi.lookup(a, fa.data(), key_hash(a, fa.data(), key_size, opt), probe.data());
            if (mate < 0) {
                ++out.removed_total;
                if (out.removed.size() <= opt.max_rows) out.removed.emplace_back(row, -1);
                continue;
            }
            ++out.matched;
            bi.fields_of(mate, fb.data());
            bool any = false;
            for (std::size_t i = 0; i < nc; ++i) {
                const Field x = fa[key_size + i], y = fb[key_size + i];
                if (cell_differs(a, x, b, y, opt)) {
                    any = true;
                    ++out.columns[i].changed;
                    if (is_absent(b, y, opt)) ++out.columns[i].blanked;
                    if (is_absent(a, x, opt)) ++out.columns[i].filled;
                }
            }
            if (any) {
                ++out.changed_total;
                if (out.changed.size() <= opt.max_rows) out.changed.emplace_back(row, mate);
            }
        }
    };

    auto a_side = [&] {
        std::vector<std::thread> workers;
        std::vector<std::exception_ptr> failures(join_ways);
        workers.reserve(join_ways - 1);
        auto guarded = [&](unsigned p) {
            try {
                a_range(p);
            } catch (...) {
                failures[p] = std::current_exception();
            }
        };
        for (unsigned p = 1; p < join_ways; ++p) {
            try {
                workers.emplace_back(guarded, p);
            } catch (const std::system_error&) {
                guarded(p);
            }
        }
        guarded(0);
        for (auto& w : workers) w.join();
        for (const auto& f : failures)
            if (f) std::rethrow_exception(f);

        // Merged in range order, so the rows kept under the cap are the same
        // rows one thread would have kept.
        for (const Part& part : parts) {
            matched += part.matched;
            for (std::size_t i = 0; i < nc; ++i) {
                r.columns[i].changed += part.columns[i].changed;
                r.columns[i].blanked += part.columns[i].blanked;
                r.columns[i].filled += part.columns[i].filled;
            }
            for (const auto& [row, mate] : part.changed) changed.push(row, mate);
            changed.total += part.changed_total - static_cast<std::int64_t>(part.changed.size());
            for (const auto& [row, mate] : part.removed) removed.push(row, mate);
            removed.total += part.removed_total - static_cast<std::int64_t>(part.removed.size());
        }
    };
    auto b_side = [&] {
        std::vector<Field> fb(width), probe(width);
        for (int row : bi.first_rows()) {
            bi.fields_of(row, fb.data());
            if (ai.lookup(b, fb.data(), key_hash(b, fb.data(), key_size, opt), probe.data()) < 0)
                added.push(row, -1);
        }
    };
    {
        std::exception_ptr failure;
        std::thread worker([&] {
            try {
                b_side();
            } catch (...) {
                failure = std::current_exception();
            }
        });
        try {
            a_side();
        } catch (...) {
            worker.join();
            throw;
        }
        worker.join();
        if (failure) std::rethrow_exception(failure);
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
