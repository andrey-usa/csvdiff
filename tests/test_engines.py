"""Every installed engine must return exactly the same result.

The DuckDB engine is the reference. `counts` and `columns` are the contract CI
enforces at scale; the row sections are compared here too, because the examples
are small enough that any ordering or NULL-handling difference shows up.
"""
import os

import pytest

from csvdiff import Options, compare, engines

EX = os.path.join(os.path.dirname(__file__), "..", "examples")
A, B = os.path.join(EX, "orders_2026-08.csv"), os.path.join(EX, "orders_2026-09.csv")
SECTIONS = ("counts", "columns", "changed", "added", "removed", "dup_a", "dup_b")

CONTRACT = engines.available(contract_only=True)
OTHERS = [e for e in CONTRACT if e != "duckdb"]


def options(**kw):
    return Options(key=["order_id", "line_no"], ignore=["updated_at"], tolerance=0.005, **kw)


@pytest.mark.skipif("duckdb" not in CONTRACT, reason="DuckDB is the reference engine")
@pytest.mark.parametrize("engine", OTHERS)
def test_matches_duckdb(engine):
    reference = compare(A, B, options(engine="duckdb"))
    other = compare(A, B, options(engine=engine))
    for section in SECTIONS:
        assert other[section] == reference[section], f"{engine} differs in {section}"


@pytest.mark.parametrize("engine", CONTRACT)
def test_normalisation_and_identity(engine):
    """Normalisation applies to keys and values, and a file equals itself."""
    r = compare(A, A, options(engine=engine, trim=True, ignore_case=True, empty_is_null=True))
    assert r["counts"]["changed"] == r["counts"]["added"] == r["counts"]["removed"] == 0


@pytest.mark.parametrize("engine", CONTRACT)
def test_exports_are_identical(engine, tmp_path):
    """--export-dir output is byte-for-byte the same whichever engine wrote it."""
    out = tmp_path / engine
    compare(A, B, options(engine=engine, export_dir=str(out)))
    reference = tmp_path / "duckdb"
    if engine != "duckdb":
        compare(A, B, options(engine="duckdb", export_dir=str(reference)))
    for name in ("added.csv", "removed.csv", "changed.csv"):
        assert (out / name).read_bytes() == (reference / name).read_bytes(), name


def test_unknown_engine_is_reported():
    from csvdiff.engine import CompareError

    with pytest.raises(CompareError):
        compare(A, B, options(engine="nope"))


@pytest.fixture
def tricky(tmp_path):
    """NULL spellings that CSV readers disagree about, plus a NULL in the key."""
    a = tmp_path / "a.csv"
    b = tmp_path / "b.csv"
    a.write_text('k,v,w,n\n1,"",x,1\n2,,y,2\n3, ,z,3\n,keyless,q,4\n5,NA,r,5\n', encoding="utf-8")
    b.write_text('k,v,w,n\n1,a,x,1\n2,"",y,2\n3,,z,3\n,keyless,q,4\n5,NA,r,9\n', encoding="utf-8")
    return str(a), str(b)


NORMALISATION = [
    {},
    {"trim": True},                                   # '  ' trims to '', which is not NULL
    {"trim": True, "empty_is_null": True},            # ... unless this is set
    {"ignore_case": True, "tolerance": 0.5},
]


@pytest.mark.skipif("duckdb" not in CONTRACT, reason="DuckDB is the reference engine")
@pytest.mark.parametrize("engine", OTHERS)
@pytest.mark.parametrize("normalisation", NORMALISATION, ids=lambda n: ",".join(n) or "plain")
def test_null_handling_matches_duckdb(engine, normalisation, tricky):
    a, b = tricky
    reference = compare(a, b, Options(key=["k"], engine="duckdb", **normalisation))
    other = compare(a, b, Options(key=["k"], engine=engine, **normalisation))
    for section in SECTIONS:
        assert other[section] == reference[section], f"{engine} differs in {section}"
