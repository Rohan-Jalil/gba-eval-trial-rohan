#!/usr/bin/env bash
# Copy this project's Claude Code session transcripts into logs/, redacting
# credentials, API keys, tokens, email addresses and unrelated private information
# (other projects on the build machine), and write logs/README.md.
#
#   scripts/export-logs.sh            # reads ${CLAUDE_CONFIG_DIR:-~/.claude}/projects
#   LOGS_DIR=/tmp/x scripts/export-logs.sh   # write somewhere else (dry run)
#
# Sessions exported (found by ID in any project folder):
#   architect  005f9e94-f8e5-4ebe-a362-77fc748b7f0d  wrote PLAN.md, CLAUDE.md, agents, EXECUTOR_PROMPTS.md
#   executor   ec739230-fb4b-467b-9709-e53c38e370f4  ran phases 0-6, plus its subagents/ folder
# Run it last: the executor transcript keeps growing while that session is open.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROJECTS="${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects"
OUT="${LOGS_DIR:-$ROOT/logs}"   # LOGS_DIR overrides, e.g. for a dry run
[ -d "$PROJECTS" ] || { echo "error: $PROJECTS not found (set CLAUDE_CONFIG_DIR)" >&2; exit 1; }
mkdir -p "$OUT"

python3 -I - "$PROJECTS" "$OUT" <<'PY'
import json, pathlib, re, sys

projects, out = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
SESSIONS = [
    ("architect", "005f9e94-f8e5-4ebe-a362-77fc748b7f0d",
     "Architect session (terminal 2): wrote PLAN.md, CLAUDE.md, the agent definitions and EXECUTOR_PROMPTS.md."),
    ("executor", "ec739230-fb4b-467b-9709-e53c38e370f4",
     "Executor session (terminal 3): ran phases 0-6 as integrator, launched worker and reviewer agents."),
]

# Secrets and unrelated private information are redacted. Each pattern has a label used in the README counts.
PATTERNS = [
    ("anthropic-api-key", re.compile(r"sk-ant-[A-Za-z0-9_\-]{10,}")),
    ("openai-style-key", re.compile(r"\bsk-[A-Za-z0-9]{20,}\b")),
    ("github-token", re.compile(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}\b|\bgithub_pat_[A-Za-z0-9_]{20,}\b")),
    ("aws-access-key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("slack-token", re.compile(r"\bxox[abprs]-[A-Za-z0-9\-]{10,}\b")),
    ("bearer-token", re.compile(r"(?i)\bbearer\s+[A-Za-z0-9._\-]{16,}")),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}\.[A-Za-z0-9_\-]{10,}")),
    # key=value style secrets: the value must look like a secret (16+ chars, no spaces).
    ("credential-assignment", re.compile(
        r"(?i)\b(api[_-]?key|access[_-]?token|auth[_-]?token|refresh[_-]?token|secret|password|passwd)"
        r"(\\?[\"']?\s*[:=]\s*\\?[\"']?)([A-Za-z0-9/+_.\-]{16,})")),
    ("email", re.compile(r"(?:(?<=\\[nrt])|(?<![\\A-Za-z0-9._%+\-]))[A-Za-z0-9._%+\-]+@[A-Za-z0-9.\-]+\.[A-Za-z]{2,}\b")),
    # Unrelated private information: other projects on the build machine seen in docker listings.
    ("unrelated-private", re.compile(r"(?<!\d)\d{12}\.dkr\.ecr\.[a-z0-9-]+\.amazonaws\.com(?:/[A-Za-z0-9_./\-]+)?")),
    ("unrelated-private", re.compile(
        r"(?:s2c-mcp-db-gateway|rds-mydumper|influx-backup|s2c-mobile[\w-]*|sockudo/sockudo|sockudo"
        r"|tusproject/tusd|minio/minio|apache/kafka|phpmyadmin|bp-dev[\w-]*|nai-pulse[\w-]*|ricerule[\w-]*)\b")),
]

def redact(text, counts):
    for label, pattern in PATTERNS:
        def replace(match):
            counts[label] = counts.get(label, 0) + 1
            if label == "credential-assignment":
                return match.group(1) + match.group(2) + "[REDACTED:credential]"
            return f"[REDACTED:{label}]"
        text = pattern.sub(replace, text)
    return text

def find(session_id):
    hits = sorted(projects.glob(f"*/{session_id}.jsonl"))
    if not hits:
        sys.exit(f"error: transcript {session_id}.jsonl not found under {projects}")
    return hits[0]

rows = []
for role, session_id, description in SESSIONS:
    source = find(session_id)
    sources = [(source, out / f"{role}-{session_id[:8]}.jsonl", description)]
    subagents = source.parent / session_id / "subagents"
    if subagents.is_dir():
        for path in sorted(subagents.iterdir()):
            if path.is_file():
                target = out / f"{role}-{session_id[:8]}-subagents" / path.name
                sources.append((path, target, f"Subagent of the {role} session ({path.suffix[1:]})"))
    for src, dst, what in sources:
        counts = {}
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_text(redact(src.read_text(encoding="utf-8", errors="replace"), counts), encoding="utf-8")
        rows.append((dst.relative_to(out), what, counts))
        print(f"{dst.relative_to(out)}: " + (", ".join(f"{k} x{v}" for k, v in counts.items()) or "nothing redacted"))

lines = [
    "# Claude Code session logs",
    "",
    "Exported by `scripts/export-logs.sh` from the local Claude Code transcripts (JSONL, one event per line).",
    "Redacted: credentials, API keys, tokens, email addresses, and unrelated private information (names and a",
    "cloud account ID of other projects running on the build machine, seen in docker listings), replaced by",
    "`[REDACTED:<kind>]`. Nothing else was changed or removed. Token usage per session is in `usage.md`",
    "(`scripts/token-usage.sh`).",
    "",
    "| File | What it is | Redacted |",
    "|---|---|---|",
]
for path, what, counts in rows:
    redacted = ", ".join(f"{k} x{v}" for k, v in sorted(counts.items())) or "nothing"
    lines.append(f"| `{path}` | {what} | {redacted} |")
(out / "README.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"wrote {out / 'README.md'}")
PY
