# Notes: decisions and agent usage

Newest entries at the bottom of each section. Every decision or change of approach gets a row:
date, what, why.

## Decisions

| Date | What | Why |
|---|---|---|
| 2026-11-05 | Single Rust crate `gba_emu` (`cdylib` + `rlib`), one module per subsystem | One file per owner keeps parallel agents from editing the same file; `rlib` lets `cargo test` and a native debug binary run without wasm. |
| 2026-11-05 | Phase 0 fixes all shared contracts (bus, IO dispatch, IRQ, cycle timing, ABI) before any parallel work | Parallel agents otherwise invent incompatible interfaces and merges conflict. |
| 2026-11-05 | Fast loop = `cargo test` + `run_rom` + `scripts/grade-subset.sh`; full grade (~10 min) only at milestones | A full grade measured ~10 min; 2-test subset measured ~7 s. |
| 2026-11-05 | Phases 0-3 must-have, 4-6 stretch | Time-limited; a verified 0-3 beats a broken 0-6. Graphics carry 60% of the score through replays. |
| 2026-11-05 | No reading of `gba-eval/candidates`, Mesen2 source or other emulators; enforced by CLAUDE.md, `.claude/settings.json` deny rules and the reviewer agent | Client requires original code. Deny rules are a guard rail only; the reviewer is the real check. |
| 2026-11-05 | Audio must be produced at Mesen2's rate (`32768 << SOUNDBIAS resolution`) | The grader analyses candidate audio at the reference's rate (`lockstep/src/lib.rs`), so a different rate distorts the spectrum. |
| 2026-11-05 | Score ceiling noted as 0.96 | `comprehensive` has weight 0.20 in procedural but no testcases, so it always scores 0. |
| 2026-11-05 | Baseline to beat: 0.5345 overall (replay 0.596, procedural 0.486, audio 0.399) | Full grade of `candidates/gba-core/gba_core_shim.wasm`, run as a binary only; its source was not read. |
| 2026-11-05 | Worker branches merged with `--no-ff` after a fresh-context reviewer; max 2 review/fix rounds, then escalate to the human | Keeps each worker's changes traceable and stops agents looping on the same failure. |
| 2026-11-05 | Each milestone is tagged `phase-<N>` and committed with `results/milestones/phase-<N>/`; agents never push, the human pushes | Gives a known-good point to roll back to; the human controls what reaches GitHub. |
| 2026-11-05 | Added "Code readability" rules to CLAUDE.md (descriptive names, commented gbatek register names, doc comment naming the gbatek section, small functions); reviewer checks them | Client and human must be able to read and verify the code; the gbatek references make review against the spec fast. |

## Agent and model usage per phase

| Date | Phase | Agent (role) | Model | Branch | Outcome |
|---|---|---|---|---|---|
| 2026-11-05 | planning | main session (planning, scripts) | claude-opus-5-5 | main | PLAN.md, CLAUDE.md, scripts, agent definitions, executor prompts |
