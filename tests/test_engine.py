import os, subprocess, sys
from csvdiff import Options, compare, render, is_identical

EX = os.path.join(os.path.dirname(__file__), "..", "examples")
A, B = os.path.join(EX, "orders_2026-08.csv"), os.path.join(EX, "orders_2026-09.csv")


def test_counts_and_tolerance():
    r = compare(A, B, Options(key=["order_id", "line_no"], ignore=["updated_at"], tolerance=0.005))
    c = r["counts"]
    assert c["a_rows"] == 800 and c["b_rows"] == 799
    assert c["added"] == 10 and c["removed"] == 12 and c["changed"] == 87
    assert c["a_dup_keys"] == 1 and c["b_dup_keys"] == 1
    assert {x["name"]: x["changed"] for x in r["columns"]}["unit_price"] == 0   # inside tolerance
    assert r["meta"]["only_in_b"] == ["carrier"]
    assert "<html" in render(r)


def test_identical_and_exit_codes(tmp_path):
    assert is_identical(compare(A, A, Options(key=["order_id", "line_no"])))
    env = {**os.environ, "PYTHONPATH": os.path.join(EX, "..")}
    rc = subprocess.run([sys.executable, "-m", "csvdiff", "compare", A, A, "-k", "order_id,line_no",
                         "-o", str(tmp_path / "r.html")], env=env).returncode
    assert rc == 0
    rc = subprocess.run([sys.executable, "-m", "csvdiff", "compare", A, B, "-k", "missing",
                         "-o", str(tmp_path / "r.html")], env=env, capture_output=True).returncode
    assert rc == 2
