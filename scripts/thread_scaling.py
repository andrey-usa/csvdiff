"""Is the C++/C CSV gap in the serial path or the parallel one?

The instruction profile is single-threaded and says 1.33x. The published table is
four-threaded and says 2.46x. Those cannot both describe the same thing, so this
measures the same pair at 1 and 4 threads and looks at how each port scales.
"""
import subprocess, time, resource, statistics, sys, collections

PORTS = {"C":   "/home/user/csvdiff/c/csvdiff",
         "C++": "/home/user/csvdiff/cpp/build/csvdiff"}
A, B = "/home/user/ab4m/c_a.csv", "/home/user/ab4m/c_b.csv"
BASE = ["compare", A, B, "-k", "account_id,txn_id", "-i", "updated_at"]
ROUNDS = int(sys.argv[1]) if len(sys.argv) > 1 else 7

def run(exe, threads):
    r0 = resource.getrusage(resource.RUSAGE_CHILDREN); t0 = time.perf_counter()
    subprocess.run([exe, *BASE, "--threads", str(threads)],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    t1 = time.perf_counter(); r1 = resource.getrusage(resource.RUSAGE_CHILDREN)
    return t1 - t0, (r1.ru_utime - r0.ru_utime) + (r1.ru_stime - r0.ru_stime)

res = collections.defaultdict(list)
for r in range(ROUNDS):
    order = list(PORTS.items())
    if r % 2: order = order[::-1]          # rotate who goes first
    for threads in (1, 4):
        for name, exe in order:
            res[(name, threads)].append(run(exe, threads))

print(f"{'port':<5} {'thr':>3} {'wall':>7} {'cpu':>7} {'cores':>6}")
for (name, thr), vals in sorted(res.items()):
    w = statistics.median(v[0] for v in vals)
    c = statistics.median(v[1] for v in vals)
    print(f"{name:<5} {thr:>3} {w:>6.2f}s {c:>6.2f}s {c/w:>5.2f}x")

print()
for thr in (1, 4):
    cw = statistics.median(v[0] for v in res[("C", thr)])
    pw = statistics.median(v[0] for v in res[("C++", thr)])
    cc = statistics.median(v[1] for v in res[("C", thr)])
    pc = statistics.median(v[1] for v in res[("C++", thr)])
    print(f"{thr} thread(s): C++/C wall {pw/cw:.3f}x   cpu {pc/cc:.3f}x")
for name in PORTS:
    w1 = statistics.median(v[0] for v in res[(name, 1)])
    w4 = statistics.median(v[0] for v in res[(name, 4)])
    print(f"{name:<5} scales {w1/w4:.2f}x from 1 to 4 threads")
