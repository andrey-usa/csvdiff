"""Engine registry.

Every engine is a `compare(a_path, b_path, opt) -> dict` returning the result
contract documented at the top of `csvdiff/engine.py`. They are interchangeable
by design: CI asserts that all installed engines produce identical `counts` and
`columns` for the same inputs, so picking one is purely a performance and
deployment decision (see BENCHMARKS.md).

`koala` is the exception and is marked `contract=False`: it wraps the
third-party koala-diff Rust tool, which infers column types and has no notion of
ignored columns or normalisation. It is kept as an external reference point in
the benchmark, not as a drop-in engine.
"""
from __future__ import annotations

import importlib
import importlib.util
from dataclasses import dataclass
from typing import Any, Callable


@dataclass(frozen=True)
class Spec:
    name: str
    module: str            # module holding the implementation
    func: str              # callable inside it
    requires: str | None   # third-party import that must be present
    contract: bool         # returns the full, comparable result contract
    blurb: str


SPECS: dict[str, Spec] = {s.name: s for s in (
    Spec("duckdb", "csvdiff.engine", "_compare_duckdb", "duckdb", True,
         "SQL hash join, streams from disk and spills; the only out-of-core engine"),
    Spec("polars", "csvdiff.engines.polars_engine", "compare", "polars", True,
         "Rust dataframes, lazy scan then in-memory join"),
    Spec("datafusion", "csvdiff.engines.datafusion_engine", "compare", "datafusion", True,
         "Apache DataFusion, the same SQL as DuckDB on an Arrow engine"),
    Spec("arrow", "csvdiff.engines.arrow_engine", "compare", "pyarrow", True,
         "pyarrow CSV reader and Acero hash join, no query planner"),
    Spec("pandas", "csvdiff.engine", "_compare_pandas", "pandas", True,
         "row-by-row fallback, in memory"),
    Spec("python", "csvdiff.engines.python_engine", "compare", None, True,
         "standard library only: streaming csv reader and a dict hash join"),
    Spec("koala", "csvdiff.engines.koala_engine", "compare", "koala_diff", False,
         "third-party koala-diff (Rust); reference point only, not contract-complete"),
)}

# Order `--engine auto` walks. DuckDB stays first: it is the only engine that
# survives files larger than RAM.
AUTO_ORDER = ("duckdb", "polars", "datafusion", "arrow", "pandas", "python")

NAMES = ["auto", *SPECS]


def is_available(name: str) -> bool:
    """Is this engine's dependency installed? Checked without importing it —
    building `--engine` help must not drag polars, pyarrow and pandas into every
    invocation of the CLI."""
    spec = SPECS[name]
    if spec.requires is None:
        return True
    try:
        return importlib.util.find_spec(spec.requires) is not None
    except (ImportError, ValueError):
        return False


def available(contract_only: bool = False) -> list[str]:
    """Installed engines, in `auto` preference order then the rest."""
    rest = [n for n in SPECS if n not in AUTO_ORDER]
    return [n for n in (*AUTO_ORDER, *rest)
            if is_available(n) and (SPECS[n].contract or not contract_only)]


def resolve_auto() -> str:
    for name in AUTO_ORDER:
        if is_available(name):
            return name
    return "python"


def get(name: str) -> Callable[..., dict[str, Any]]:
    from ..engine import CompareError

    spec = SPECS.get(name)
    if spec is None:
        raise CompareError(f"Unknown engine: {name}. Available: {', '.join(available())}")
    try:
        module = importlib.import_module(spec.module)
    except ImportError as e:
        raise CompareError(f"The {name} engine needs `pip install {spec.requires}` ({e}).") from e
    return getattr(module, spec.func)
