"""Does each port charge for row samples nobody asked for?

If a port builds its sample lists unconditionally, asking for --json costs it
almost nothing extra. If it gates them, --json is visibly more expensive. This
does not read any code, which is the point.
"""
import subprocess, time, statistics, collections
PORTS = {"C":    ("/home/user/csvdiff/c/csvdiff", []),
         "C++":  ("/home/user/csvdiff/cpp/build/csvdiff", []),
         "Rust": ("/home/user/csvdiff/rust/target/release/csvdiff", ["-o","/dev/null"]),
         "Zig":  ("/home/user/csvdiff/zig/zig-out/bin/csvdiff", [])}
A, B = "/home/user/ab4m/c_a.csv", "/home/user/ab4m/c_b.csv"
K = ["-k","account_id,txn_id","-i","updated_at","--threads","4"]

def run(exe, extra, js):
    t0 = time.perf_counter()
    subprocess.run([exe,"compare",A,B,*K,*extra] + (["--json","/dev/null"] if js else []),
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return time.perf_counter() - t0

res = collections.defaultdict(list)
for r in range(7):
    items = list(PORTS.items()); items = items[r%4:] + items[:r%4]
    for name,(exe,extra) in items:
        for js in (False, True):
            res[(name,js)].append(run(exe,extra,js))

print(f"{'port':<5} {'no --json':>10} {'--json':>9} {'json costs':>11}")
for name in PORTS:
    n = statistics.median(res[(name,False)]); j = statistics.median(res[(name,True)])
    print(f"{name:<5} {n:>9.3f}s {j:>8.3f}s {(j-n)/n*100:>10.0f}%")
print("\nA port that already skips the samples shows a large cost for --json.")
print("One that builds them regardless shows little.")
