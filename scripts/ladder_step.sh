#!/usr/bin/env bash
# One rung of the scale ladder, as its own CI step.
#
# A step per size rather than one step for the whole ladder, because a single
# step is one collapsed block in the Actions UI: you cannot see which sizes
# passed until the whole thing ends, and if it ends by being killed you cannot
# see anything at all. A step per size makes the progress visible as it happens
# and makes the ceiling literally the first red tick.
#
# The step goes red when its size fails, which stops the ones after it -- that
# is the ladder ending, not an error to fix. The artifact upload runs with
# `if: always()` so the results still come back.
#
# Everything else comes from the environment, so the workflow says the size and
# nothing else.
set -euo pipefail

size=${1:?usage: ladder_step.sh SIZE}
: "${LADDER_BIN:?}" "${LADDER_PORT:?}" "${LADDER_DATA:?}"

mkdir -p out
python3 scripts/bench_scale.py \
    --binary "$LADDER_BIN" \
    --label "$LADDER_PORT" \
    --runner "${LADDER_RUNNER:-}" \
    ${LADDER_GEN:+--generator "$LADDER_GEN"} \
    --sizes "$size" \
    --repeats "${LADDER_REPEATS:-1}" \
    --threads "${LADDER_THREADS:-4}" \
    --data-dir "$LADDER_DATA" \
    --json-out "out/ceiling-${LADDER_PORT}-${LADDER_RUNNER:-host}-${size}.json" \
    --fail-exit
