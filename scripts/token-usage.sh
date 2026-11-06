#!/usr/bin/env bash
# Sum token usage per session from exported transcripts into logs/usage.md.
#
#   scripts/token-usage.sh [logs-dir]      # default: logs/ (run scripts/export-logs.sh first)
#
# Each API response is counted once (deduplicated by message id; a response that is
# split over several transcript lines repeats the same usage). Subagent transcripts
# are summed into their parent session and also listed on their own.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
LOGS="${1:-$ROOT/logs}"
[ -d "$LOGS" ] || { echo "error: $LOGS not found; run scripts/export-logs.sh first" >&2; exit 1; }

python3 -I - "$LOGS" <<'PY'
import collections, json, pathlib, sys

logs = pathlib.Path(sys.argv[1])
FIELDS = ["input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens", "output_tokens"]

def usage_of(path):
    """Token totals and model counts for one transcript, one entry per message id."""
    totals, models, seen = collections.Counter(), collections.Counter(), set()
    for line in path.open(encoding="utf-8", errors="replace"):
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        message = event.get("message") if isinstance(event, dict) else None
        if event.get("type") != "assistant" or not isinstance(message, dict) or "usage" not in message:
            continue
        key = message.get("id") or event.get("requestId") or event.get("uuid")
        if key in seen:
            continue
        seen.add(key)
        for field in FIELDS:
            totals[field] += message["usage"].get(field) or 0
        models[message.get("model", "unknown")] += 1
    totals["responses"] = len(seen)
    return totals, models

def row(name, totals, models):
    cells = [f"{totals[f]:,}" for f in FIELDS + ["responses"]]
    total = sum(totals[f] for f in FIELDS)
    model_list = ", ".join(f"{m} ({n})" for m, n in models.most_common())
    return f"| {name} | " + " | ".join(cells) + f" | {total:,} | {model_list} |"

header = ["| Transcript | Input | Cache write | Cache read | Output | Responses | All tokens | Models (responses) |",
          "|---|---|---|---|---|---|---|---|"]
sessions, details = [], []
for main in sorted(logs.glob("*.jsonl")):
    totals, models = usage_of(main)
    sub_dir = logs / (main.stem + "-subagents")
    sub_rows = []
    if sub_dir.is_dir():
        for sub in sorted(sub_dir.glob("*.jsonl")):
            sub_totals, sub_models = usage_of(sub)
            totals.update(sub_totals)
            models.update(sub_models)
            sub_rows.append(row(f"{sub_dir.name}/{sub.name}", sub_totals, sub_models))
    sessions.append(row(main.stem + (" (incl. subagents)" if sub_rows else ""), totals, models))
    if sub_rows:
        details += [f"### {main.stem} subagents", ""] + header + sub_rows + [""]

text = ["# Token usage per session", "",
        "Summed from the exported transcripts by `scripts/token-usage.sh`. Each API response counted once.",
        "\"All tokens\" = input + cache write + cache read + output.", ""]
text += header + sessions + [""] + details
(logs / "usage.md").write_text("\n".join(text) + "\n", encoding="utf-8")
print("\n".join(text))
PY
