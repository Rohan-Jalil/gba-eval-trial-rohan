#!/usr/bin/env bash
# Milestone grade (~10 min): build, run the full gba-eval grader, append the
# score to scores.tsv, and save summary.json + the score row under
# results/milestones/<phase>/ (committed). Commit your code first so the
# score maps to a commit.
#
#   scripts/grade-full.sh phase-1 "CPU gate passed"
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
GBA_EVAL="${GBA_EVAL:-/var/www/html/test-projects/gba-eval}"
[ $# -ge 1 ] || { sed -n '2,8p' "$0"; exit 2; }
PHASE="$1"; NOTE="${2:-}"
cd "$ROOT"
[ -z "$(git status --porcelain)" ] || echo "warning: uncommitted changes; score will not match the commit" >&2

cargo build --release --lib --target wasm32-unknown-unknown
COMMIT="$(git rev-parse --short HEAD)"
NAME="gba-emu-$PHASE-$COMMIT-$(date +%Y%m%dT%H%M%S)"
"$GBA_EVAL/quickstart/grade.sh" "$ROOT/target/wasm32-unknown-unknown/release/gba_emu.wasm" "$NAME"

HEADER=$'date\tphase\tcommit\toverall\treplay\tprocedural\taudio\tnote'
ROW="$(python3 -I - "$GBA_EVAL/results/$NAME/summary.json" "$PHASE" "$COMMIT" "$NOTE" <<'PY'
import json, sys, datetime
s = json.load(open(sys.argv[1])); sec = s["sections"]
print("\t".join([datetime.date.today().isoformat(), sys.argv[2], sys.argv[3], f"{s['overall']:.4f}",
    *(f"{sec[k]['score']:.4f}" for k in ("replay", "procedural", "audio")), sys.argv[4]]))
PY
)"
[ -f scores.tsv ] || echo "$HEADER" > scores.tsv
echo "$ROW" >> scores.tsv

MILESTONE="results/milestones/$PHASE"
mkdir -p "$MILESTONE"
cp "$GBA_EVAL/results/$NAME/summary.json" "$MILESTONE/summary.json"
printf '%s\n%s\n' "$HEADER" "$ROW" > "$MILESTONE/score.tsv"
echo "$ROW"
echo "saved $MILESTONE/ (commit it together with scores.tsv)"
