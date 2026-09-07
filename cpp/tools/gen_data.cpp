// Writes the same deterministic pair of CSV files as scripts/gen_data.py and the
// four other generators, byte for byte, using every core.
//
// The recipe is identical because it has to be: parity.yml checks that all the
// generators emit the same bytes, so a benchmark number from one is comparable
// with a number from another. What differs here is only how the bytes are
// produced. Every row is a pure function of its index -- the drift buckets come
// from a hash of the row number, and money is carried in integer cents so no
// language's rounding rule can enter into it -- which means a row can be
// formatted without knowing any other row. So chunks of rows are formatted in
// parallel and written in order.
//
// Rows are variable width, so a thread cannot know its file offset in advance.
// Rather than guess, the work goes in waves: N threads each fill their own
// buffer, the buffers are written in order, and the next wave starts. Memory is
// bounded by the wave, not by the file.
//
//   cd cpp && make tools/gen-data
//   cpp/build/gen-data --rows 10m --out-dir data --prefix 10m

#include <fcntl.h>
#include <unistd.h>

#include <algorithm>
#include <charconv>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

namespace {

constexpr const char* kColumns =
    "account_id,txn_id,posting_date,value_date,currency,amount,fee,balance,status,channel,"
    "region,branch_code,product_code,counterparty,quantity,rate,category,risk_flag,note,"
    "updated_at\n";

const char* const kStatus[] = {"posted", "pending", "settled", "reversed"};
const char* const kChannel[] = {"branch", "online", "mobile", "atm", "wire"};
const char* const kRegion[] = {"EMEA", "NA", "APAC", "LATAM"};
const char* const kCurrency[] = {"USD", "EUR", "GBP", "JPY"};
const char* const kCategory[] = {"retail", "corporate", "treasury", "cards", "loans"};

// Drift buckets, against a 0..9999 hash bucket per row.
constexpr int kChgStatus = 300, kChgAmount = 150, kChgBalance = 150, kChgValueDate = 30;
constexpr std::int64_t kRemovedMod = 1000, kAddedRatio = 1000, kDupMod = 10000;

// 240 dates from 2026-01-01, precomputed once: the other generators get these
// from a date library, and this one only needs the same strings out.
struct Days {
    char text[240][11];
    Days() {
        // Written a digit at a time rather than with snprintf: the compiler
        // cannot see that month and day are bounded, so a format string draws a
        // truncation warning that -Werror turns into a build failure.
        static const int len[] = {31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
        int month = 0, day = 1;
        for (int i = 0; i < 240; ++i) {
            char* p = text[i];
            *p++ = '2'; *p++ = '0'; *p++ = '2'; *p++ = '6'; *p++ = '-';
            *p++ = static_cast<char>('0' + (month + 1) / 10);
            *p++ = static_cast<char>('0' + (month + 1) % 10);
            *p++ = '-';
            *p++ = static_cast<char>('0' + day / 10);
            *p++ = static_cast<char>('0' + day % 10);
            *p = '\0';
            if (++day > len[month]) {
                day = 1;
                ++month;
            }
        }
    }
};
const Days kDays;

// splitmix-style mix, matching the other implementations bit for bit.
std::uint64_t mix(std::int64_t i, std::int64_t salt, std::int64_t seed) {
    std::uint64_t x = static_cast<std::uint64_t>(i * 31 + salt + seed);
    x = (x ^ (x >> 30)) * 0xBF58476D1CE4E5B9ULL;
    x = (x ^ (x >> 27)) * 0x94D049BB133111EBULL;
    return x ^ (x >> 31);
}

int mod(std::int64_t i, std::int64_t salt, std::int64_t seed, int m) {
    return static_cast<int>(mix(i, salt, seed) % static_cast<std::uint64_t>(m));
}

char* put(char* p, const char* s) {
    const std::size_t n = std::strlen(s);
    std::memcpy(p, s, n);
    return p + n;
}

char* put_int(char* p, std::int64_t v) {
    const auto done = std::to_chars(p, p + 24, v);
    return done.ptr;
}

// Left-pads to `width` with zeroes, which is what the other generators do with
// a format string.
char* put_pad(char* p, std::int64_t v, int width) {
    char tmp[24];
    const auto done = std::to_chars(tmp, tmp + sizeof tmp, v);
    const int n = static_cast<int>(done.ptr - tmp);
    for (int i = n; i < width; ++i) *p++ = '0';
    std::memcpy(p, tmp, static_cast<std::size_t>(n));
    return p + n;
}

// An amount held in cents, written as a two-decimal number.
char* put_money(char* p, std::int64_t cents) {
    if (cents < 0) {
        *p++ = '-';
        cents = -cents;
    }
    p = put_int(p, cents / 100);
    *p++ = '.';
    return put_pad(p, cents % 100, 2);
}

// One row of one side. `p` must have at least 256 bytes of room.
char* row(char* p, std::int64_t i, bool b, std::int64_t seed) {
    const int bucket = mod(i, 0, seed, 10000);
    std::int64_t amount_cents = mod(i, 21, seed, 900000000) - 100000000;
    std::int64_t balance_cents = mod(i, 31, seed, 2000000000);
    const char* st = kStatus[mod(i, 11, seed, 4)];
    const char* value_date = kDays.text[mod(i, 41, seed, 240)];

    if (b) {
        if (bucket < kChgStatus) {
            st = kStatus[(mod(i, 11, seed, 4) + 1) % 4];
        } else if (bucket < kChgStatus + kChgAmount) {
            amount_cents += 1234;
        } else if (bucket < kChgStatus + kChgAmount + kChgBalance) {
            balance_cents = (balance_cents * 101 + 50) / 100;  // +1%, half up, in cents
        }
        if (bucket < kChgValueDate) value_date = "";
    }

    p = put(p, "ACC-");
    p = put_pad(p, (i * 7919) % 250000, 8);
    p = put(p, ",TXN-");
    p = put_pad(p, i, 11);
    *p++ = ',';
    p = put(p, kDays.text[mod(i, 1, seed, 240)]);
    *p++ = ',';
    p = put(p, value_date);
    *p++ = ',';
    p = put(p, kCurrency[mod(i, 51, seed, 4)]);
    *p++ = ',';
    p = put_money(p, amount_cents);
    *p++ = ',';
    p = put_money(p, mod(i, 61, seed, 5000));
    *p++ = ',';
    p = put_money(p, balance_cents);
    *p++ = ',';
    p = put(p, st);
    *p++ = ',';
    p = put(p, kChannel[mod(i, 71, seed, 5)]);
    *p++ = ',';
    p = put(p, kRegion[mod(i, 81, seed, 4)]);
    p = put(p, ",BR");
    p = put_pad(p, mod(i, 91, seed, 900) + 100, 4);
    p = put(p, ",P");
    p = put_pad(p, mod(i, 101, seed, 5000), 5);
    p = put(p, ",CP-");
    p = put_pad(p, mod(i, 111, seed, 90000), 6);
    *p++ = ',';
    p = put_int(p, mod(i, 121, seed, 500) + 1);
    p = put(p, ",0.");
    p = put_pad(p, mod(i, 131, seed, 1200), 4);
    *p++ = ',';
    p = put(p, kCategory[mod(i, 141, seed, 5)]);
    *p++ = ',';
    *p++ = mod(i, 151, seed, 20) == 0 ? 'Y' : 'N';
    p = put(p, ",batch ");
    p = put_int(p, i % 997 + 1);
    p = put(p, " line ");
    p = put_int(p, i % 53 + 1);
    *p++ = ',';
    p = put(p, b ? "2026-09-01 02:15:00" : "2026-08-01 02:15:00");
    *p++ = '\n';
    return p;
}

// Every row index in [from, to) as it appears in file A, then in file B. The
// two sides differ: B drops one row in a thousand and repeats one in ten
// thousand, both decided by the index alone, which is what lets a chunk be
// built without seeing the chunks before it.
void fill(std::int64_t from, std::int64_t to, std::int64_t rows, std::int64_t seed,
          std::string& a, std::string& b) {
    a.clear();
    b.clear();
    char scratch[256];
    for (std::int64_t i = from; i < to; ++i) {
        a.append(scratch, static_cast<std::size_t>(row(scratch, i, false, seed) - scratch));
        if (i % kRemovedMod != 7)
            b.append(scratch, static_cast<std::size_t>(row(scratch, i, true, seed) - scratch));
        if (i % kDupMod == 3 && i < rows / 2)
            b.append(scratch, static_cast<std::size_t>(row(scratch, i, true, seed) - scratch));
    }
}

void write_all(int fd, const char* data, std::size_t n, const char* path) {
    while (n > 0) {
        const ssize_t wrote = ::write(fd, data, n);
        if (wrote <= 0) {
            std::fprintf(stderr, "error: cannot write %s\n", path);
            std::exit(2);
        }
        data += wrote;
        n -= static_cast<std::size_t>(wrote);
    }
}

std::int64_t parse_rows(const std::string& s) {
    std::string t;
    for (char c : s)
        if (c != '_' && c != ',') t += static_cast<char>(std::tolower(c));
    if (t.empty()) return -1;
    std::int64_t mult = 1;
    if (t.back() == 'k') mult = 1000;
    else if (t.back() == 'm') mult = 1000000;
    else if (t.back() == 'g') mult = 1000000000;
    if (mult > 1) t.pop_back();
    try {
        return static_cast<std::int64_t>(std::stod(t) * static_cast<double>(mult) + 0.5);
    } catch (...) {
        return -1;
    }
}

}  // namespace

int main(int argc, char** argv) {
    std::string rows_arg = "10k", out_dir = "data", prefix;
    std::int64_t seed = 7;
    unsigned threads = 0;
    for (int i = 1; i < argc; ++i) {
        const std::string f = argv[i];
        auto next = [&]() -> std::string { return i + 1 < argc ? argv[++i] : ""; };
        if (f == "--rows" || f == "-n") rows_arg = next();
        else if (f == "--out-dir" || f == "-o") out_dir = next();
        else if (f == "--prefix") prefix = next();
        else if (f == "--seed") seed = std::stoll(next());
        else if (f == "--threads") threads = static_cast<unsigned>(std::stoul(next()));
        else if (f == "-h" || f == "--help") {
            std::printf("usage: gen-data --rows 10m --out-dir DIR [--prefix P] [--threads N]\n");
            return 0;
        }
    }
    const std::int64_t rows = parse_rows(rows_arg);
    if (rows <= 0) {
        std::fprintf(stderr, "error: --rows must be a positive number\n");
        return 2;
    }
    if (prefix.empty()) prefix = rows_arg;
    if (threads == 0) threads = std::max(1u, std::thread::hardware_concurrency());

    const std::string a_path = out_dir + "/" + prefix + "_a.csv";
    const std::string b_path = out_dir + "/" + prefix + "_b.csv";
    const int fa = ::open(a_path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
    const int fb = ::open(b_path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (fa < 0 || fb < 0) {
        std::fprintf(stderr, "error: cannot create the output files in %s\n", out_dir.c_str());
        return 2;
    }
    write_all(fa, kColumns, std::strlen(kColumns), a_path.c_str());
    write_all(fb, kColumns, std::strlen(kColumns), b_path.c_str());

    // Enough rows per chunk that a thread has real work, few enough that a wave
    // of them stays comfortably inside the page cache.
    const std::int64_t per_chunk = std::max<std::int64_t>(1 << 14, rows / (threads * 16));
    std::vector<std::string> a_buf(threads), b_buf(threads);
    for (unsigned t = 0; t < threads; ++t) {
        a_buf[t].reserve(static_cast<std::size_t>(per_chunk) * 200);
        b_buf[t].reserve(static_cast<std::size_t>(per_chunk) * 200);
    }

    for (std::int64_t start = 0; start < rows; start += per_chunk * threads) {
        std::vector<std::thread> workers;
        unsigned live = 0;
        for (unsigned t = 0; t < threads; ++t) {
            const std::int64_t from = start + static_cast<std::int64_t>(t) * per_chunk;
            if (from >= rows) break;
            const std::int64_t to = std::min(from + per_chunk, rows);
            ++live;
            if (t == 0) continue;  // this thread takes chunk 0 itself
            workers.emplace_back([&, t, from, to] { fill(from, to, rows, seed, a_buf[t], b_buf[t]); });
        }
        if (live == 0) break;
        fill(start, std::min(start + per_chunk, rows), rows, seed, a_buf[0], b_buf[0]);
        for (auto& w : workers) w.join();
        for (unsigned t = 0; t < live; ++t) {
            write_all(fa, a_buf[t].data(), a_buf[t].size(), a_path.c_str());
            write_all(fb, b_buf[t].data(), b_buf[t].size(), b_path.c_str());
        }
    }

    // The tails: duplicate keys appended to A, and rows only in B.
    const std::int64_t dup_extra = std::max<std::int64_t>(1, rows / kDupMod);
    const std::int64_t added = std::max<std::int64_t>(1, rows / kAddedRatio);
    std::string tail;
    char scratch[256];
    for (std::int64_t i = 0; i < dup_extra; ++i)
        tail.append(scratch, static_cast<std::size_t>(row(scratch, i, false, seed) - scratch));
    write_all(fa, tail.data(), tail.size(), a_path.c_str());
    tail.clear();
    for (std::int64_t i = rows; i < rows + added; ++i)
        tail.append(scratch, static_cast<std::size_t>(row(scratch, i, true, seed) - scratch));
    write_all(fb, tail.data(), tail.size(), b_path.c_str());

    ::close(fa);
    ::close(fb);
    std::printf("c++: %lld rows x 20 columns on %u threads\n", static_cast<long long>(rows),
                threads);
    return 0;
}
