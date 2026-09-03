"""Named comparison profiles from csvdiff.toml (searched in CWD, then ~/.config/csvdiff/).

[profiles.orders]
key      = ["order_id", "line_no"]
compare  = ["qty", "price", "status"]   # omit for all common non-key columns
ignore   = ["updated_at"]
trim     = true
tolerance = 0.005
"""
from __future__ import annotations

import os
import tomllib
from dataclasses import fields

from .engine import Options

SEARCH = ["csvdiff.toml", os.path.expanduser("~/.config/csvdiff/csvdiff.toml")]


def load_config(path: str | None = None) -> dict:
    for p in ([path] if path else SEARCH):
        if p and os.path.isfile(p):
            with open(p, "rb") as f:
                return tomllib.load(f)
    return {}


def options_from(profile: dict | None, overrides: dict) -> Options:
    """Merge a profile (may be None) with CLI/email/form overrides. Overrides win when not None."""
    merged = dict(profile or {})
    for k, v in overrides.items():
        if v is not None:
            merged[k] = v
    allowed = {f.name for f in fields(Options)}
    merged.setdefault("key", [])
    return Options(**{k: v for k, v in merged.items() if k in allowed})


def parse_list(s):
    """'a, b ,c' -> ['a','b','c']; list passthrough; None -> None."""
    if s is None or isinstance(s, list):
        return s
    return [x.strip() for x in str(s).split(",") if x.strip()]
