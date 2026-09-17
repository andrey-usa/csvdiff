"""Does --memory-cap cost time? Paired, one machine, one binary, one input.

The ladder harness sets RLIMIT_DATA and the native one does not, and that is the
only material difference between the two. The ladder is also the only run where
Rust came last on ndjson. This holds everything else constant and varies the cap.
"""
import subprocess, time, resource, statistics, sys, collections

PORTS = {
    "Rust": "/home/user/csvdiff/rust/target/release/csvdiff",
    "C":    "/home/user/csvdiff/c/csvdiff",
}
A, B = "/home/user/ab4m/n_a.ndjson", "/home/user/ab4m/n_b.ndjson"
K = ["compare", A, B, "-k", "account_id,txn_id", "-i", "updated_at"]
EXTRA = {"Rust": ["-o", "/dev/null"], "C": []}

total_mb = int(subprocess.run(["awk", "/^MemTotal:/ { printf \"%d\", $2 / 1024 }", "/proc/meminfo"],
                              capture_output=True, text=True).stdout)
CAP_MB = total_mb - 2048                      # exactly what bench-ladder computes
print(f"host has {total_mb} MB; the ladder's cap would be {CAP_MB} MB\n")

def run(port, capped):
    limit = CAP_MB * 1024 * 1024
    def preexec():
        resource.setrlimit(resource.RLIMIT_DATA, (limit, limit))
    r0 = resource.getrusage(resource.RUSAGE_CHILDREN)
    t0 = time.perf_counter()
    subprocess.run([PORTS[port], *K, *EXTRA[port]],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   preexec_fn=preexec if capped else None)
    t1 = time.perf_counter()
    r1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    return t1 - t0, (r1.ru_utime - r0.ru_utime) + (r1.ru_stime - r0.ru_stime)

ROUNDS = int(sys.argv[1]) if len(sys.argv) > 1 else 9
for port in PORTS:
    res = collections.defaultdict(list); ratios = []
    for r in range(ROUNDS):
        arms = [("capped", True), ("free", False)]
        for name, cap in (arms if r % 2 else arms[::-1]):      # rotate
            w, c = run(port, cap); res[name].append((w, c))
        ratios.append(res["capped"][-1][0] / res["free"][-1][0])
    ratios.sort()
    lo = statistics.median(ratios[:len(ratios)//2]); hi = statistics.median(ratios[(len(ratios)+1)//2:])
    fw = statistics.median(w for w, _ in res["free"]); cw = statistics.median(w for w, _ in res["capped"])
    print(f"{port:<5} free {fw:5.2f}s   capped {cw:5.2f}s   "
          f"capped/free median {statistics.median(ratios):.3f}  mid half {lo:.3f}-{hi:.3f}  "
          f"slower in {sum(1 for x in ratios if x > 1.0)}/{ROUNDS}", flush=True)
