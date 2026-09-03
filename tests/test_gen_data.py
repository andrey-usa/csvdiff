"""The generator is part of CI, so its drift rates are asserted like any other behaviour."""
import subprocess
import sys

from csvdiff import Options, compare

KEY = ["account_id", "txn_id"]


def gen(tmp_path, rows="10k", engine="python"):
    subprocess.run([sys.executable, "scripts/gen_data.py", "-n", rows, "-o", str(tmp_path),
                    "--engine", engine], check=True)
    return str(tmp_path / f"{rows}_a.csv"), str(tmp_path / f"{rows}_b.csv")


def test_shape_and_drift(tmp_path):
    a, b = gen(tmp_path)
    r = compare(a, b, Options(key=KEY, ignore=["updated_at"]))
    c = r["counts"]
    assert len(r["meta"]["compared"]) == 17            # 20 columns - 2 key - 1 ignored
    assert c["a_keys"] == 10_000 and c["b_keys"] == 10_000
    assert c["added"] == 10 and c["removed"] == 10     # 0.1% each
    assert c["a_dup_keys"] == 1 and c["b_dup_keys"] == 1
    assert 0.055 < c["changed"] / c["matched"] < 0.065  # ~6% of matched rows differ
    by = {x["name"]: x for x in r["columns"]}
    assert by["status"]["changed"] > by["amount"]["changed"] > 0
    assert by["value_date"]["blanked"] == by["value_date"]["changed"] > 0
    assert by["note"]["changed"] == 0                  # untouched columns stay untouched


def test_deterministic(tmp_path):
    a1, b1 = gen(tmp_path / "one")
    a2, b2 = gen(tmp_path / "two")
    assert open(a1, "rb").read() == open(a2, "rb").read()
    assert open(b1, "rb").read() == open(b2, "rb").read()
