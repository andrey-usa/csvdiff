"""csvdiff: parameterised composite-key CSV comparison with a self-contained HTML report."""
from .engine import Options, compare, is_identical, CompareError
from .report import render

__version__ = "1.0.0"
__all__ = ["Options", "compare", "is_identical", "CompareError", "render"]
