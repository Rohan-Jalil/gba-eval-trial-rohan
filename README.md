# gba-emu

An original Game Boy Advance emulator in Rust, compiled to `wasm32-unknown-unknown` and graded by the
[gba-eval](https://github.com/mechanize-work/gba-eval) benchmark against the Mesen2 reference. It was built
by Claude Code agents (model `claude-opus-5-5`) working from a written plan, with every change reviewed by a
separate agent before it was merged.

**Submission version: tag `phase-6`** (code at commit `b556f0e`). The prebuilt wasm is in
[`dist/gba_emu.wasm`](dist/gba_emu.wasm), with its checksum in [`dist/SHA256SUMS`](dist/SHA256SUMS). It is
byte-identical to a fresh build of the tag.

## Score

Full grader runs, one per milestone (`scores.tsv`, details in `results/milestones/phase-N/summary.json`):

| Milestone | Commit | Overall | Replay | Procedural | Audio | Main change |
|---|---|---|---|---|---|---|
| Baseline (gba-core) | — | 0.5345 | 0.596 | 0.486 | 0.399 | reference point given by the benchmark |
| phase-1 | db2cba4 | 0.1303 | 0.1143 | 0.3083 | 0.0000 | CPU (ARM + THUMB) and basic display |
| phase-2 | 8d55cb9 | 0.4326 | 0.4673 | 0.7611 | 0.0000 | Interrupts/halt, timers, DMA, bus wait states, open bus |
| phase-3 | 9f4a767 | 0.6213 | 0.7818 | 0.7611 | 0.0000 | Sprites, affine/bitmap BGs, windows, blending, mosaic |
| phase-4 | e551e06 | 0.7976 | 0.7806 | 0.7611 | 0.8852 | Sound: PSG channels, FIFO A/B, mixer |
| phase-5 | a31eece | 0.8476 | 0.8648 | 0.7611 | 0.8827 | Compatibility: DMA start timing, display power-on, SRAM |
| **phase-6** | **b556f0e** | **0.8476** | **0.8648** | **0.7611** | **0.8827** | Performance (2x fuel headroom), reset determinism |
| Ceiling | — | 0.96 | 1.0 | 0.8 | 1.0 | the `comprehensive` subsystem has weight but no tests |

Phase 0 (skeleton) has no full grade. Every section of the final version beats the baseline. 15 of the 16
procedural tests score 1.0; the exception is `mgba-multiply-long` (see "Changes of approach").

## Setup

Tools used (versions on the build machine):

| Tool | Version |
|---|---|
| rustc / cargo | 1.99.1, with target `wasm32-unknown-unknown` |
| Docker | 29.7.2 (runs the `gba-eval-grader` image) |
| git / git-lfs | 2.43.0 / 3.4.1 (gba-eval keeps its reference cache in LFS) |
| Python 3 | used by the scripts (standard library only) |
| Claude Code | 2.1.318, model `claude-opus-5-5`; only needed to rerun the agent workflow |

Lay the two repos out so gba-eval is at `../../gba-eval` relative to this repo, or set `GBA_EVAL`:

```bash
mkdir work && cd work
git clone https://github.com/mechanize-work/gba-eval
(cd gba-eval && git submodule update --init --recursive && git lfs install && git lfs pull)
mkdir trial && git clone https://github.com/Rohan-Jalil/gba-eval-trial-rohan trial/gba-emu
cd trial/gba-emu
rustup target add wasm32-unknown-unknown
# or, for any other layout: export GBA_EVAL=/absolute/path/to/gba-eval
```

Build and test:

```bash
cargo test                                                    # 129 unit tests, incl. reset determinism
cargo build --release --lib --target wasm32-unknown-unknown   # -> target/wasm32-unknown-unknown/release/gba_emu.wasm
cargo run --release --bin run_rom -- <rom.gba> <frames> out.ppm [out.wav]   # native debug runner
```

The first grade builds the `gba-eval-grader` docker image (see gba-eval's `quickstart/README.md`).

## Grading

```bash
# Full grade (~10 min): builds, grades, appends to scores.tsv, saves results/milestones/<name>/
scripts/grade-full.sh phase-x "note"

# Subset: only the named testcases, results in results/<run-name>/ (read the per-testcase lines)
scripts/grade-subset.sh target/wasm32-unknown-unknown/release/gba_emu.wasm my-run memory aw-arm-alu tonc-snd1
```

Run graders one at a time; parallel graders can run the machine out of memory (wrap them in
`flock /tmp/gba-emu-grader.lock ...`).

**Verify independently.** Build any milestone tag and grade it with gba-eval's own script, without this repo's
scripts:

```bash
git checkout phase-6        # or phase-1 ... phase-5
cargo build --release --lib --target wasm32-unknown-unknown
"$GBA_EVAL/quickstart/grade.sh" "$PWD/target/wasm32-unknown-unknown/release/gba_emu.wasm" gba-emu-phase-6
# or grade the committed artifact: "$GBA_EVAL/quickstart/grade.sh" "$PWD/dist/gba_emu.wasm" gba-emu-dist
sha256sum -c dist/SHA256SUMS   # from dist/
```

## Repository layout

```
src/                 the emulator (see Architecture)
src/bin/run_rom.rs   native runner: N frames of a ROM to .ppm (+ .wav)
assets/              gba_bios_stub.bin, copied from gba-eval/spec (embedded with include_bytes!)
dist/                release wasm of tag phase-6 + SHA256SUMS
scripts/             grade-full.sh, grade-subset.sh, gba-eval-path.sh (default GBA_EVAL),
                     export-logs.sh, token-usage.sh
results/milestones/  summary.json and score row of each full grade (other results/ output is ignored)
scores.tsv           one row per milestone
PLAN.md              the build plan: scoring analysis, architecture and contracts, phases, workflow, risks
CLAUDE.md            binding rules for every agent: originality, readability, ownership, git, grading budget
EXECUTOR_PROMPTS.md  the prompt for each phase and Procedures A (worker) and B (milestone)
NOTES.md             decision log (date, what, why) and the agent/model used per phase
TIMELOG.md           my own hours
.claude/agents/      one definition per worker role, plus `reviewer`
.claude/settings.json  deny rules (other emulators' source, web access, git push)
logs/                exported Claude Code transcripts and token usage (after scripts/export-logs.sh)
```

## Architecture

One crate, `gba_emu`, built as `cdylib` (the wasm) and `rlib` (native tests and `run_rom`). Each subsystem
lives in its own file, so parallel agents never edited the same file:

| File | Component |
|---|---|
| `lib.rs` | the 10 ABI exports (`spec/ABI.md`), thin wrappers over one global `Gba`; ROM, framebuffer and audio buffers live outside `Gba` so their pointers stay valid |
| `gba.rs` | `Gba`: CPU + bus, `run_frame` main loop, event routing between components |
| `bus.rs` | memory map, 8/16/32-bit access, wait states (WAITCNT, sequential/non-sequential), open bus, BIOS protection |
| `io.rs` | IO register dispatch by address range, readable-bit masks, 8/32-bit split and merge |
| `irq.rs` | IE/IF/IME, `request(IrqSource)`, HALTCNT |
| `cpu/` | `mod.rs` registers, modes, banking, exceptions, `step`; `alu.rs` shifter and flags; `arm.rs`, `thumb.rs` decoders |
| `ppu/` | `mod.rs` line timing, DISPCNT/DISPSTAT, affine reference points; `bg_text.rs`, `bg_affine.rs`, `bitmap.rs`, `obj.rs` layer renderers; `effects.rs` the compositor (windows, blending, mosaic) |
| `apu/` | `mod.rs` mixer, SOUNDBIAS, sample rate; `psg.rs` channels 1-4; `fifo.rs` direct sound A/B |
| `timer.rs`, `dma.rs`, `keypad.rs`, `cart.rs` | timers, 4 DMA channels, KEYINPUT/KEYCNT, ROM and SRAM |

Shared contracts, fixed in Phase 0 and changed only by the integrator:

- **Bus:** the CPU only talks to the bus (`read8/16/32`, `write8/16/32`); each access adds its cycle cost,
  drained by the CPU per instruction. The bus owns every component except the CPU.
- **IO:** owners expose `io_read16(offset)` and `io_write16(offset, value, mask)`, where the mask marks
  the bytes really written (a read-modify-write would break IF acknowledge, HALTCNT and the FIFOs).
- **Interrupts:** any component calls `irq.request(source)`; only the CPU consumes them.
- **Timing:** `run_frame` runs 280,896 cycles (228 lines x 1232) from line 0 through VBlank, measured to
  match Mesen2's frame boundary; `emu_load_rom`/`emu_reset` run 5 hidden frames so frame 0 lines up with the
  reference (the grader burns 6 warm-up frames on Mesen2 and none on us). Since Phase 6, PPU, timers and APU
  are caught up only at their next event or on an IO access, with identical results.
- **Events:** PPU HBlank/VBlank to DMA, timer overflow to the FIFOs, FIFO refill to DMA, all as plain
  method calls in `gba.rs`.
- **Graphics:** every layer renders one scanline into `[LayerPixel; 240]` (`LineLayers`); only the
  compositor merges them.
- **Audio:** output at Mesen2's rate, `32768 << SOUNDBIAS bits 14-15`, with an exact sample count per frame.

## Workflow

Two Claude Code sessions on `claude-opus-5-5`, plus one private AI assistant session:

1. **Private assistant session (not included).** Some prompts were drafted with a separate AI assistant;
   that session is private and not included.
2. **Architect (terminal 2).** It studied gba-eval and wrote `PLAN.md`, `CLAUDE.md`, the agent definitions in
   `.claude/agents/`, `EXECUTOR_PROMPTS.md` and the grading scripts.
3. **Executor (terminal 3).** It acted as the integrator and ran Phases 0-6 from `EXECUTOR_PROMPTS.md`:
   - it launched parallel worker agents, each in its own git worktree and branch (`phaseN/<role>`), owning only its files;
   - it merged a branch only after a fresh `reviewer` agent with no author context had checked it against gbatek, the ownership rules and for signs of copied code;
   - each merge passed `cargo test`, the wasm build and the smoke set, and each phase ended with a full grade and a tag.

Logs for the architect and executor (with all subagents) sessions are in `logs/`. An abandoned first
session (one response, restarted in a clean folder) is not included.
Token usage per session is in `logs/usage.md`, with the architect session listed separately; it covers
these sessions only, not the private assistant session.

Every branch, including failed and unmerged attempts, is kept in the history: for example
`phase5/spout-gameplay` and `phase5/heartwrench-prefetch`. The worktrees have been removed.

### Agents, skills and prompts

- **Agent definitions:** `.claude/agents/*.md`, one per worker role (skeleton-builder, cpu-core, cpu-arm, cpu-thumb,
  ppu-basic, irq-halt, timers, dma, ppu-sprites, ppu-affine-bitmap, ppu-effects, apu-psg, apu-fifo-mixer,
  compat-timing, perf-hardening) plus `reviewer`.
- **Phase prompts and procedures:** `EXECUTOR_PROMPTS.md`, Prompts 0-6 and Procedures A and B.
- **Rules every agent follows:** `CLAUDE.md`.
- **Guard rails:** `.claude/settings.json` deny rules.
- **Skills:** no custom skills were written for this project.
- **Per-phase record:** `NOTES.md` lists which agents ran on which model, with review rounds and outcomes.

## Significant changes of approach

All of these are logged in `NOTES.md` with date and reason.

- **mgba gate moved (Phase 1 to 2).** The three mgba-suite CPU tests need interrupts and DMA before they can
  show anything, so they moved to the Phase 2 gate. There they pass, except multiply-long.
- **Multiply-long carry accepted as a loss.** gbatek only says the carry flag is "destroyed". A carry rule
  fitted to the test ROM's screens was flagged by review as an originality risk and removed. Cost: about
  0.008 overall.
- **Replay gate rule amended (Phase 4).** Replay came out 0.0012 below Phase 3, all of it spout's video, while
  audio went from 0 to 0.885. I accepted the exception instead of reverting to 0.62 overall.
- **Grader serialisation and grading budget.**
  - Parallel graders ran out of memory in Phase 3, so graders now run one at a time behind `flock`, and reviewers don't grade.
  - Phase 5 timing workers kept iterating, so `CLAUDE.md` now caps each worker at 5 grader runs. After that it commits its best measured version and reports.
- **Phase 5 stopped after round 1.**
  - goodboy turned out to be a DMA start-timing bug rather than graphics: its video went from 0.08 to 1.0.
  - The two timing workers' changes (CPU branch refill, cartridge prefetch) each made another replay worse on their own. They are kept on branches rather than merged.

## Known limitations and next steps

- **Weakest replays:** spout (video 0.29), heartwrench (0.58) and celeste (0.73). They drift out of sync over
  time because of cycle timing. The most promising next step is to combine CPU pipeline-refill cycles
  (gbatek 2S+1N per branch) with the Game Pak prefetch buffer: together they matched heartwrench's first 900
  frames exactly. Both are on unmerged branches.
- **Key latching:** the reference may apply key input at VBlank start. Trying that made spout's intro match
  exactly.
- **Audio:**
  - piugba audio is 0.68: our output runs about 957 samples longer, cause unknown.
  - spout and heartwrench audio follow their video drift.
- **Save hardware:** only SRAM. FLASH, EEPROM and the GPIO/rumble port are not implemented.
- **Timing model:** there is no cycle-exact scheduler.
  - The DMA start delay and IRQ entry cycles aren't charged.
  - A halted CPU wakes up to 16 cycles late.
  - Several timer and DMA constants were measured against the grader and must be re-checked if CPU timing changes.
- **Smaller review items** are deferred and listed in `NOTES.md`:
  - vertical mosaic when it is switched on partway through a block;
  - mosaic on affine layers;
  - a few readability splits.

## Originality

- **No existing emulator code was used, read or paraphrased.** All code was written by the agents from
  gbatek (the GBA, ARM and THUMB chapters in `gba-eval/spec/`), the ABI spec, the BIOS stub and the grader's
  outputs (scores, `.ref.png` vs `.cand.png`, measured behaviour of the test ROMs).
- **Guards:**
  - `CLAUDE.md` forbids reading gba-eval's baseline candidate, Mesen2 and other emulators, and forbids per-ROM hacks.
  - `.claude/settings.json` denies reads of those folders and denies web access.
  - Every branch was reviewed by a fresh agent that looked specifically for copied structure, names or tables. Two such concerns came up (a fitted multiply carry rule, Game Boy documentation details in the PSG), and both were removed.
  - The deny rules only cover Claude Code agents started from this repo, so they are a guard rail, not proof.

## Omitted or external

- **gba-eval is not included.** That repository (grader, test corpus, ROMs, replays, reference cache, gbatek,
  `reference/mesen.wasm`) is separate and has its own licences. Clone it as described in Setup.
- **Docker** is needed to grade (the `gba-eval-grader` image).
- **A Claude Code subscription** is needed to rerun the agent workflow. Building, testing and grading need
  only the tools in Setup.
