#!/usr/bin/env bash
# Fast feedback: grade a wasm against only the testcases you name.
#
#   scripts/grade-subset.sh <wasm> <run-name> <testcase-id>...
#   scripts/grade-subset.sh target/wasm32-unknown-unknown/release/gba_emu.wasm cpu aw-arm-alu mgba-carry
#
# Builds a throwaway corpus (filtered testcases.json + the real ROMs,
# replays and reference cache mounted read-only) and runs the same
# gba-eval-grader docker image as the full grade. Results land in
# results/<run-name>/ in this repo. Section scores in summary.json are
# meaningless for a subset (missing subsystems count as 0); read the
# per-testcase entries instead.
set -euo pipefail

GBA_EVAL="${GBA_EVAL:-$("$(dirname "$0")/gba-eval-path.sh")}"
[ $# -ge 3 ] || { sed -n '2,12p' "$0"; exit 2; }
WASM="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; NAME="$2"; shift 2
[ -f "$WASM" ] || { echo "error: $WASM not found" >&2; exit 1; }

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/results/$NAME"; SUBSET="$(mktemp -d)"
trap 'rm -rf "$SUBSET"' EXIT
mkdir -p "$OUT"

python3 -I - "$GBA_EVAL/corpus/testcases.json" "$SUBSET/testcases.json" "$@" <<'PY'
import json, sys
src, dst, *ids = sys.argv[1:]
data = json.load(open(src))
cases = data if isinstance(data, list) else data["testcases"]
picked = [c for c in cases if c["id"] in ids]
missing = set(ids) - {c["id"] for c in picked}
if missing:
    sys.exit(f"unknown testcase id(s): {', '.join(sorted(missing))}")
json.dump(picked if isinstance(data, list) else {**data, "testcases": picked}, open(dst, "w"), indent=1)
PY
cp "$GBA_EVAL/corpus/grader.yaml" "$SUBSET/"

docker run --rm \
    -v "$GBA_EVAL":/repo:ro \
    -v "$SUBSET":/subset \
    -v "$GBA_EVAL/corpus/roms":/subset/roms:ro \
    -v "$GBA_EVAL/corpus/replays":/subset/replays:ro \
    -v "$GBA_EVAL/corpus/reference-cache":/subset/reference-cache:ro \
    -v "$(dirname "$WASM")":/wasm:ro \
    -v "$OUT":/out \
    gba-eval-grader /wasm/"$(basename "$WASM")" /subset/ /out

python3 -I - "$OUT" <<'PY'
import json, sys
s = json.load(open(f"{sys.argv[1]}/summary.json"))
for tc, r in sorted(s["per_testcase"].items()):
    print(f"{tc:28} video={r.get('video_score')!s:22} audio={r.get('audio_score')}")
PY
