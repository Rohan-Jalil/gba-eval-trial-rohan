# Executor prompts

Paste these one at a time into a Claude Code session opened in this repo (the "executor"). Start a
prompt only when the previous one has finished and its gate passed. Prompts 0-3 are must-have;
4-6 are stretch and start only after the Phase 3 milestone is verified.

Each prompt refers to the two procedures below. The executor reads them from this file.

---

## Procedure A: run, review and merge one worker

1. Launch the worker with the Agent tool: `subagent_type` = the role name, `isolation: "worktree"`.
   The task text must include:
   - its branch name `phase<N>/<role>`, which it creates with `git switch -c` in its worktree;
   - the `main` commit it starts from;
   - any extra instructions from the phase prompt;
   - the grading budget from CLAUDE.md ("Grading budget": at most 5 grader runs, serialised with `flock`).
   If a worker is still experimenting after its budget, send it a hard stop with SendMessage: commit the
   best measured version and report.
2. When the worker reports, launch a **new** `reviewer` agent, never a continued one, so its context is
   fresh. Give it only the branch name. Do not pass on the worker's report or reasoning.
3. If the verdict is `changes needed`, or `ORIGINALITY: concerns`, or there are `major` readability findings, send the findings back to the worker
   with SendMessage. It fixes them and commits, then a new reviewer runs. Stop after 2 rounds: log the
   open findings in NOTES.md and ask the human.
4. Merge into `main`: `git merge --no-ff phase<N>/<role> -m "chore(<scope>): merge phase<N>/<role>"`.
5. On `main`, run `cargo test`, the wasm build and the smoke set:
   `scripts/grade-subset.sh target/wasm32-unknown-unknown/release/gba_emu.wasm main-<role> aw-arm-alu aw-thumb-alu memory timer-reload dma-priority tonc-snd1`.
   If a smoke score dropped, fix it or revert the merge before doing anything else.
6. Add a row to the NOTES.md "Agent and model usage" table: date, phase, role, model, branch, outcome
   (gate scores, review rounds). Commit it: `docs(notes): phase <N> <role> merged`.

## Procedure B: close a phase (milestone)

1. Make sure `main` is committed and clean.
2. Run `scripts/grade-full.sh phase-<N> "<one-line note>"` (about 10 minutes).
3. Check the phase gate against the full-grade numbers. If it fails, do not close the phase: fix, or
   report to the human.
4. Commit the results: `git add scores.tsv results/milestones/phase-<N> && git commit -m "chore(results): phase <N> milestone <overall score>"`.
5. Tag the milestone: `git tag phase-<N>`.
6. Add the phase's decisions and changes of approach to NOTES.md (date, what, why) and commit
   (`docs(notes): phase <N> decisions`).
7. Do not push. The human pushes.
8. Report to the human:
   - the score row and how it compares with the previous milestone and with the 0.534 baseline;
   - which agents ran;
   - what is still open.
   Then stop and wait for the next prompt.

Commits always use Conventional Commits and the repo's git identity (Rohan Jalil), as CLAUDE.md says.

---

## Prompt 0: skeleton and contracts [MUST-HAVE]

```
Read CLAUDE.md, PLAN.md and EXECUTOR_PROMPTS.md (Procedures A and B).

Preflight, and stop if any check fails:
- `cargo --version` works and `rustup target list --installed` includes wasm32-unknown-unknown.
- `docker image inspect gba-eval-grader` succeeds.
- `git config user.name` is "Rohan Jalil" and `git config user.email` is "rohan.jalil@gmail.com".

Phase 0: run one `skeleton-builder` worker on branch phase0/skeleton, following Procedure A. It builds
every file in PLAN.md section 2 as stubs with the agreed contracts, the 10 ABI exports, the embedded
BIOS stub, and src/bin/run_rom.rs.

Extra check before merging: compare our frame boundary with Mesen2. With a ROM whose first frames change
visibly (e.g. $GBA_EVAL/corpus/roms/test/tonc/m3-demo.gba), work out where
the reference's emu_run_frame ends relative to VBlank, using the grader's .ref.png output, and make our
run_frame end at the same point. Log the finding in NOTES.md.

Gate: wasm builds; `scripts/grade-subset.sh ... p0 aw-arm-alu timer-reload` runs with no "trapped" skip;
`cargo test` passes and includes a bus round-trip test; run_rom writes a .ppm.

Then follow Procedure B with phase-0, but skip the full grade: the scores would all be ~0. Instead,
commit the subset run's per-test output as results/milestones/phase-0/subset.txt.
```

**What it does:** It checks the toolchain and git identity, then has one agent build the empty
skeleton and the shared contracts every later agent relies on. It also sets the frame boundary to
match Mesen2's, because getting that wrong would offset every frame in every replay.

---

## Prompt 1: CPU and basic display [MUST-HAVE]

```
Read CLAUDE.md, PLAN.md (Phase 1) and EXECUTOR_PROMPTS.md (Procedures A and B).

Phase 1, run as two waves following Procedure A for every worker:

Wave 1 (in parallel): `cpu-core` on phase1/cpu-core and `ppu-basic` on phase1/ppu-basic.
As soon as cpu-core has reviewed, merged helpers (register file, conditions, shifter, flag
helpers), start wave 2 from that main commit. ppu-basic can keep running.

Wave 2 (in parallel): `cpu-arm` on phase1/cpu-arm and `cpu-thumb` on phase1/cpu-thumb.

Merge each branch as soon as it passes review. After all four are merged, run the CPU gate:
scripts/grade-subset.sh target/wasm32-unknown-unknown/release/gba_emu.wasm p1-gate aw-arm-alu aw-arm-ldr-str aw-arm-ldm-stm aw-thumb-alu aw-thumb-ldr-str aw-thumb-ldm-stm mgba-shifter mgba-carry mgba-multiply-long

Gate: every one of those 9 tests scores at least 0.99. Also run jsmolka arm.gba and thumb.gba via run_rom:
both show all tests passed. For any failing test, send the .cand.png/.ref.png difference to the owning
worker. Allow at most 2 fix rounds per test, then log it in NOTES.md and ask the human.

Then follow Procedure B with phase-1.
```

**What it does:** It builds the CPU in two waves. The shared CPU core and the basic display go first,
then ARM and THUMB run in parallel on top of the core. The phase closes only when all 9 CPU test ROMs
pass and the full grade is saved.

---

## Prompt 2: interrupts, timers, DMA, memory [MUST-HAVE]

```
Read CLAUDE.md, PLAN.md (Phase 2) and EXECUTOR_PROMPTS.md (Procedures A and B).

Phase 2. Hand ownership of src/irq.rs to the irq-halt worker for this phase, and log that in NOTES.md.
Run in parallel, following Procedure A: `irq-halt` on phase2/irq-halt, `timers` on phase2/timers,
`dma` on phase2/dma.

While they run, do the memory/bus work yourself on main, in src/bus.rs (wait states/WAITCNT, mirroring,
open bus, misaligned reads and writes), using gbatek "GBA Memory Map", "GBA System Control" and
"ARM CPU Memory Alignments". Commit it in small conventional commits, and pass a fresh `reviewer` on
your own diff before relying on it.

Gate tests: scripts/grade-subset.sh ... p2-gate memory mgba-io-read timer-reload timer-start-stop dma-priority mgba-dma ppu-vram-mirror
plus all 9 Phase 1 tests (no regressions).

Then follow Procedure B with phase-2. Gate on the full grade: procedural section at least 0.6
(the ceiling is 0.8), and no Phase 1 test below 0.99.
```

**What it does:** It runs the interrupt, timer and DMA workers in parallel while the executor fixes
the memory details in the shared bus file. The phase closes once all 16 test ROMs are graded and the
test-ROM section reaches 0.6.

---

## Prompt 3: full graphics [MUST-HAVE]

```
Read CLAUDE.md, PLAN.md (Phase 3) and EXECUTOR_PROMPTS.md (Procedures A and B).

Phase 3. First, on main, make sure the per-line `[LayerPixel; 240]` contract from PLAN.md exists in
src/ppu/mod.rs and is used by bg_text and bitmap. Commit that as `refactor(ppu): layer line contract`
before launching workers.

Run in parallel, following Procedure A: `ppu-sprites` on phase3/ppu-sprites, `ppu-affine-bitmap` on
phase3/ppu-affine-bitmap, `ppu-effects` on phase3/ppu-effects (owns the compositor).
Merge ppu-effects last if they conflict in ppu/mod.rs.

Subset check after each merge:
scripts/grade-subset.sh ... p3-<role> spout-gameplay piugba-gameplay tonc-snd1 bulletgba-gameplay celeste-gameplay
(video scores should rise; these are long replays, so expect a few minutes).

Then follow Procedure B with phase-3. Gate on the full grade: overall above 0.534 (baseline),
replay above 0.596, procedural not lower than phase-2.

This closes the must-have scope. In the report, state clearly whether the must-have scope is
complete and verified.
```

**What it does:** It locks down the per-line layer format first, then runs sprites, scaling/bitmap
backgrounds and effects in parallel, with one agent owning the final layer merge. The must-have scope
is finished when the full grade beats the 0.534 baseline overall and in the replay section.

---

## Prompt 4: sound [STRETCH]

```
Read CLAUDE.md, PLAN.md (Phase 4) and EXECUTOR_PROMPTS.md (Procedures A and B).
Only start if tag phase-3 exists and its gate passed. Otherwise stop and say so.

Run in parallel, following Procedure A: `apu-psg` on phase4/apu-psg and `apu-fifo-mixer` on
phase4/apu-fifo-mixer. Wire the timer-overflow-to-FIFO and FIFO-empty-to-DMA events in src/gba.rs
yourself (shared file).

Subset check: scripts/grade-subset.sh ... p4 tonc-snd1 chip-advance-gameplay celeste-gameplay piugba-gameplay
(read audio_score). Verify emu_audio_rate() follows SOUNDBIAS (32768 << bits 14-15).

Then follow Procedure B with phase-4. Gate: audio section above 0.399, and overall and replay not
lower than phase-3. If the gate fails, revert to tag phase-3 behaviour (keep the branches) and
report rather than leaving main worse.
```

**What it does:** It builds the retro sound channels and the sample playback/mixer in parallel, with
the executor wiring them to the timers and DMA. If sound would lower the overall score, `main` goes
back to the Phase 3 state.

---

## Prompt 5: game compatibility and timing [STRETCH]

```
Read CLAUDE.md, PLAN.md (Phase 5) and EXECUTOR_PROMPTS.md (Procedures A and B).
Only start if tag phase-3 exists. Otherwise stop and say so.

From the latest milestone summary.json, list the replay testcases sorted by video score. Take the
three lowest. For each, run a `compat-timing` worker (in parallel, branch phase5/<testcase-id>)
whose task names that one testcase and the files it may touch (cart.rs for saves/GPIO; a timing
table you assign; never the same file to two workers at once). Follow Procedure A.

After the merges, run those three replays plus the smoke set with scripts/grade-subset.sh.
Repeat with the next three lowest if time allows. Ask the human before each new round.

Then follow Procedure B with phase-5. Gate: overall above the phase-3/phase-4 milestone, and no
replay below its previous score by more than 0.02.
```

**What it does:** It takes the three worst-scoring games and gives each to its own agent, which finds
the first frame where the game drifts from Mesen2 and fixes the hardware cause. You approve each
further round, so this phase never runs open-ended.

---

## Prompt 6: performance and hardening [STRETCH]

```
Read CLAUDE.md, PLAN.md (Phase 6) and EXECUTOR_PROMPTS.md (Procedures A and B).

Measure fuel use per frame on the slowest replay (from the latest full grade's per-testcase JSON, or
by timing run_rom natively). Run one `perf-hardening` worker on phase6/perf-hardening. Its task
names the hot paths it may change. It also adds the reset-determinism test
(run 100 frames, emu_reset, run 100 frames, framebuffer hashes equal). Follow Procedure A.

Then follow Procedure B with phase-6. Gate: every graded score equal to phase-5 (or the latest
milestone) within 0.005, determinism test passes, slowest replay has at least 2x fuel headroom.
Finish the report with a final summary: all milestone rows from scores.tsv and what remains.
```

**What it does:** It makes the emulator faster without changing its behaviour, and adds a test that
reset really returns to a clean state. It ends with a final summary of every milestone score.
