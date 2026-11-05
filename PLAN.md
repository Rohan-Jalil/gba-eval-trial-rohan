# gba-emu build plan

Goal: an original Game Boy Advance emulator in Rust, built as `wasm32-unknown-unknown`, exporting the 10
functions in `gba-eval/spec/ABI.md`, that scores above the 0.534 baseline on the gba-eval grader. The
grader's highest reachable score is 0.96, because the `comprehensive` subsystem carries weight but has no tests.

Paths in this document are relative to this repo (`gba-trial/gba-emu`). The benchmark repo is
gba-eval, referred to below as `$GBA_EVAL` (default `../../gba-eval` relative to this repo).

## Scope: must-have vs stretch

| Phases | Status | Meaning |
|---|---|---|
| 0-3 | **Must-have** | Skeleton, CPU, interrupts/timers/DMA, full graphics. Each one finished, gated and verified with a milestone grade before the next starts. |
| 4-6 | **Stretch** | Sound, game compatibility/timing, performance. Started only once Phase 3 is verified, and dropped if time runs out. |

A solid, verified 0-3 is worth more than a broken 0-6. Never start a stretch phase while a
must-have gate is failing, and never leave `main` in a state that does not build or that scores lower
than the last milestone.

---

## 1. How the score is made (what to optimise)

| Section | Weight | Tests | Scoring |
|---|---|---|---|
| Replay | 60% | 11 homebrew games with recorded input | Mean per-frame SSIM vs Mesen2 over thousands of frames. Drifting out of sync ruins every later frame. |
| Procedural | 20% | 16 test ROMs | Only the final frame counts, and it must be almost pixel-exact (pass/fail). |
| Audio | 20% | 9 of the replays | Per-frame log-mel spectrum vs Mesen2. Silence scores 0. |

Procedural subsystem weights: cpu 0.35, comprehensive 0.20 (no tests, always 0), memory 0.15,
timer 0.12, halt_irq 0.10, dma 0.08. Audio: psg 0.5, game_audio 0.5.

Consequences:
- **CPU correctness comes first.** Every other test depends on it.
- **The PPU (graphics) carries the most weight**, because all 11 replays are graphics comparisons.
- **Timing accuracy matters a lot**, because replay inputs land on fixed frame numbers. If the game boots
  a few frames later than in Mesen2, every button press lands at the wrong moment.
- **Colour precision is forgiving.** The grader quantises to 5 bits per channel, so any sensible
  5-to-8-bit colour expansion is fine.

---

## 2. Architecture and shared contracts (Phase 0, one agent, before any parallel work)

Everything that more than one agent touches is defined here first. Parallel agents then work only inside
their own files, which is what keeps merges conflict-free.

One crate, `gba_emu`, with `crate-type = ["cdylib", "rlib"]`. The `rlib` lets `cargo test` and a native
debug binary run on the host without wasm.

```
src/
  lib.rs          ABI exports only (the 10 functions), thin wrappers over a single global Gba
  gba.rs          Gba struct: owns every component, run_frame() main loop
  bus.rs          memory map: read8/16/32, write8/16/32, region dispatch, returns access cycles
  io.rs           IO register dispatch table: address range -> owning component
  irq.rs          IE / IF / IME, request(IrqSource), pending() -> bool
  scheduler.rs    cycle counter; components are stepped by "cycles elapsed"
  cpu/mod.rs      Cpu struct, register file, modes, banking, CPSR/SPSR, exceptions, step()
  cpu/alu.rs      barrel shifter, add/sub with flags, condition evaluation
  cpu/arm.rs      ARM (32-bit) decode + execute
  cpu/thumb.rs    THUMB (16-bit) decode + execute
  ppu/mod.rs      line timing, DISPCNT/DISPSTAT/VCOUNT, line compositor, framebuffer
  ppu/bg_text.rs  tiled backgrounds (modes 0-2)
  ppu/bg_affine.rs rotation/scaling backgrounds
  ppu/bitmap.rs   modes 3/4/5
  ppu/obj.rs      sprites (normal + affine)
  ppu/effects.rs  windows, blending, mosaic
  timer.rs        4 timers, prescaler, cascade, overflow IRQ, FIFO tick
  dma.rs          4 DMA channels, triggers, priority
  apu/mod.rs      mixer, SOUNDCNT/SOUNDBIAS, output buffer, sample rate
  apu/psg.rs      channels 1-4
  apu/fifo.rs     direct sound channels A/B
  keypad.rs       KEYINPUT / KEYCNT
  cart.rs         ROM, save memory (SRAM / Flash / EEPROM), GPIO
src/bin/run_rom.rs  native debug tool: run a ROM for N frames, write the frame as .ppm and audio as .wav
```

Contracts fixed in Phase 0:

1. **Bus.** `fn read32(&mut self, addr: u32) -> u32` and the other widths, plus a cycle cost for each
   access. The CPU only talks to the bus, never to components directly.
2. **IO registers.** `io.rs` maps address ranges to owners: PPU `0x000-0x05F`, APU `0x060-0x0AF`,
   DMA `0x0B0-0x0DF`, timers `0x100-0x10F`, serial `0x120-0x12F` and `0x134-0x15F` (stub: plain storage, never
   connected), keypad `0x130-0x133`, IRQ/system `0x200-0x804`. Each owner
   exposes `io_read16(offset)` and `io_write16(offset, value)`, with 8- and 32-bit accesses split or
   merged centrally in `io.rs`. Each owner starts as a stub that its agent later fills in.
3. **Interrupts.** Any component calls `irq.request(IrqSource::VBlank)` etc. Only the CPU consumes interrupts.
4. **Timing.** `cpu.step()` returns the cycles it used. `Gba::run_frame()` loops until 280,896 cycles
   (228 lines x 1232 cycles) have elapsed, advancing PPU, timers, DMA and APU by the same cycles.
   Where exactly a frame ends (start of VBlank or not) must match Mesen2. Verify this in Phase 0 by
   comparing early frames with the reference.
5. **Events between components.** The PPU signals HBlank/VBlank to DMA. Timers signal FIFO ticks to the
   APU, and the APU signals FIFO-empty to DMA. These go through plain method calls in `gba.rs`, not
   through components holding references to each other.
6. **BIOS.** `gba-eval/spec/gba_bios_stub.bin` is embedded with `include_bytes!` and mapped at
   `0x00000000`. The CPU starts at the reset vector in ARM mode and runs it as normal code.
7. **ABI.** All 10 exports exist from day one. No imports: no `std::println!` in the wasm build,
   no threads, no allocation after init on hot paths. Do not export `emu_boot_frames`.

Phase 0 deliverable: the ABI works end to end, the BIOS is mapped, and the CPU can be a stub.
`run_rom` writes a black frame. `scripts/grade-subset.sh ... smoke <any test>` runs without
"skip: trapped" (score ~0 is expected).

---

## 3. Phases

Each phase lists its agents (run in parallel), the files they own, and the test that shows it worked.
"Gate" means the integrator merges the phase only when the gate passes.

Smoke set (run on every merge, about 20 s):
`aw-arm-alu aw-thumb-alu memory timer-reload dma-priority tonc-snd1`

### Phase 0 - skeleton and contracts (1 agent) [MUST-HAVE]
- Owns: everything in section 2.
- Gate: wasm builds, subset grade runs with no trap, `cargo test` has at least one bus round-trip test.

### Phase 1 - CPU and "just enough" display (4 agents) [MUST-HAVE]
| Agent | Owns | Dev test |
|---|---|---|
| CPU core | `cpu/mod.rs`, `cpu/alu.rs` | unit tests: flags, shifter edge cases (shift by 0/32/>32), mode banking |
| ARM | `cpu/arm.rs` | `jsmolka/arm.gba` via `run_rom` (it shows a failing test number), armwrestler ARM pages |
| THUMB | `cpu/thumb.rs` | `jsmolka/thumb.gba`, armwrestler THUMB pages |
| PPU basic | `ppu/mod.rs`, `ppu/bitmap.rs`, `ppu/bg_text.rs`, `keypad.rs` | `jsmolka/ppu-hello.gba`, `tonc/m3-demo.gba`; line timing, VCOUNT, VBlank IRQ |

The CPU core agent starts first and lands the register file, `alu.rs` and the condition codes in about
the first fifth of the phase. ARM and THUMB then build on those, and the PPU agent works independently
from the start. Test ROMs need a working display, which is why basic PPU belongs in this phase.

- Gate (graded): `aw-arm-alu aw-arm-ldr-str aw-arm-ldm-stm aw-thumb-alu aw-thumb-ldr-str aw-thumb-ldm-stm
  mgba-shifter mgba-carry mgba-multiply-long` mostly at 1.0.
- Full grade at the end of this phase. First recorded milestone.

### Phase 2 - interrupts, timers, DMA, memory details (3 agents) [MUST-HAVE]
| Agent | Owns | Graded tests |
|---|---|---|
| IRQ / halt | `irq.rs` (integrator hands over ownership), CPU exception entry, HALTCNT | `ppu-vram-mirror`; dev: `nba-hw/haltcnt.gba`, `nba-hw/irq-irq-delay.gba`, `destoer/if-ack.gba`, `tonc/irq-demo.gba` |
| Timers | `timer.rs` | `timer-reload timer-start-stop`; dev: `tonc/tmr-demo.gba` |
| DMA | `dma.rs` | `dma-priority mgba-dma`; dev: `nba-hw/dma-*.gba`, `tonc/dma-demo.gba` |

The integrator handles memory and bus details in `bus.rs`: wait states, mirroring, open bus,
misaligned access. Gate tests: `memory mgba-io-read`.

- Gate: all 16 procedural tests graded, procedural section above 0.6 (ceiling 0.8).
- Full grade at the end of this phase.

### Phase 3 - full graphics (3 agents) [MUST-HAVE]
| Agent | Owns | Dev tests |
|---|---|---|
| Sprites | `ppu/obj.rs` | `tonc/obj-demo.gba`, `tonc/obj-aff.gba`, `nba-hw/ppu-sprite-hmosaic.gba` |
| Affine + bitmap | `ppu/bg_affine.rs`, `ppu/bitmap.rs` | `tonc/sbb-aff.gba`, `tonc/m7-demo.gba`, `tonc/bm-modes.gba`, `tonc/pageflip.gba`, `nba-hw/ppu-bgx.gba` |
| Effects + compositor | `ppu/effects.rs`, compositor in `ppu/mod.rs` | `tonc/win-demo.gba`, `tonc/bld-demo.gba`, `tonc/mos-demo.gba`, `tonc/prio-demo.gba`, `destoer/window-midframe.gba` |

Contract for this phase: each layer renders one scanline into a `[LayerPixel; 240]` holding colour,
priority, "is transparent" and "is semi-transparent sprite". Only the compositor merges layers.

- Gate (graded, replay subset): `spout-gameplay piugba-gameplay tonc-snd1` video scores up from Phase 2.
- Full grade at the end of this phase. The replay section should now be clearly above the baseline's 0.596.

### Phase 4 - sound (2 agents) [STRETCH]
| Agent | Owns | Graded tests |
|---|---|---|
| PSG | `apu/psg.rs` | `tonc-snd1 chip-advance-gameplay` (audio score) |
| FIFO + mixer | `apu/fifo.rs`, `apu/mod.rs` | `celeste-gameplay piugba-gameplay` (audio score); dev: `misc/gba-sound-demo-rates.gba` |

Critical: the grader analyses your audio at **Mesen2's sample rate**, which is
`32768 << SOUNDBIAS resolution bits` (32/65/131/262 kHz depending on what the game writes). Your
`emu_audio_rate()` and the number of samples per frame must follow the same rule, or the spectrum
will be wrong even if the sound is right.

- Gate: audio section above the baseline's 0.399.

### Phase 5 - game compatibility and timing (2-3 agents, one per failing game) [STRETCH]
- Cartridge saves (SRAM / Flash / EEPROM detection) and GPIO (`collie-defense` uses rumble) in `cart.rs`.
- Cycle accuracy: instruction timings (gbatek "ARM CPU Instruction Cycle Times"), wait states,
  prefetch. This is what keeps replays in sync with Mesen2.
- Method: pick the game with the lowest replay score, open its `.cand.png` and `.ref.png`, and find the
  first frame where they differ (`--emit-video` on the grader, or the `dump` tool in `$GBA_EVAL/harness`).
- Gate: every replay above 0.5, overall above 0.534.

### Phase 6 - performance and hardening (1 agent) [STRETCH]
- The fuel limit is 500M wasm instructions per frame. Measure the slowest replay and leave at least
  2x headroom.
- Determinism: `emu_reset()` must produce exactly the same state as `emu_load_rom()`.
  Add a test: run 100 frames, reset, run 100 frames, and compare the framebuffer hashes.

---

## 4. Workflow: agents, review, tracking

**Parallel agents.**
- Agent definitions live in `.claude/agents/`, one per role below plus `reviewer`. Prompts per phase
  are in `EXECUTOR_PROMPTS.md`.
- Give each agent its own git worktree and branch (`phase1/arm`, `phase1/thumb`, ...).
- An agent only edits the files it owns in the phase table. Shared files belong to the integrator
  (you, or one coordinating agent).
- Brief each agent with three things: its phase row, `CLAUDE.md`, and its gbatek sections.
- Keep it to 4 agents at most per phase. More than that and integration costs more than it saves.

**Fast feedback loop (minutes, not 10).**
- Unit tests (`cargo test`, native, seconds) for decoder and ALU edge cases.
- `cargo run --bin run_rom -- <rom> <frames> out.ppm` for any ROM, including the ungraded dev ROMs
  under `$GBA_EVAL/corpus/roms/test/`. Many of them print PASS/FAIL or a failing test number on screen.
- `scripts/grade-subset.sh <wasm> <run-name> <testcase ids...>` grades only the named testcases with
  the real grader (2 short tests took about 7 s). Read the per-testcase lines it prints. The section
  totals are meaningless for a subset.

**Milestone loop (about 10 min).** Run `scripts/grade-full.sh phase-<n> "note"` at the end of each phase,
after committing. It builds the wasm, runs the full grader, appends a row to `scores.tsv`
(date, phase, commit, overall, replay, procedural, audio, note), and saves `summary.json` plus the score row
under `results/milestones/phase-<n>/`. Commit both. All other `results/` output is git-ignored.

**Merge checklist (integrator).**
1. The branch only touches the files it owns.
2. `cargo test` and the wasm build pass.
3. The smoke set does not regress. The phase gate tests improve.
4. A review agent, which did not write the code, reads the diff against the gbatek sections. It checks
   for mistakes and for anything that looks like it came from another emulator (unusual names, comments
   or structure that the spec doesn't explain). Use a fresh context with no access to the author's chat.
5. Merge to `main`, then rerun the smoke set on `main`.

**Tracking.**
- `scores.tsv` holds one row per milestone, and `results/milestones/` holds each milestone's `summary.json`.
- `NOTES.md` logs decisions (date, what, why) and the agents and models used per phase.
- `TIMELOG.md` is the human's own hours log.
- Keep the `results/` folders of the last good run per phase, so you can diff `.cand.png` files when
  something regresses.

---

## 5. Risks and likely problems

1. **CPU bugs look like graphics bugs.** A wrong flag in one instruction can show up as a broken sprite
   three phases later. Finish Phase 1 properly, using the ungraded jsmolka ROMs, before moving on.
2. **Replay drift.** Small timing errors shift the moment input lands, and the game diverges. This is
   the biggest score risk and the hardest to debug. Mitigate by checking frame alignment in Phase 0 and
   doing cycle timing in Phase 5.
3. **Frame boundary mismatch.** If `emu_run_frame` stops at a different point in the frame than Mesen2,
   every frame is offset. Check this against the reference early.
4. **Audio rate mismatch.** See Phase 4. Easy to get silently wrong.
5. **Games hanging on save hardware.** Many games wait forever for a Flash or EEPROM reply. A black
   screen on a replay usually means this.
6. **Shared-file conflicts** between parallel agents. Prevented by the ownership table. Expect
   contract changes anyway and route them through the integrator.
7. **Originality.**
   - Models have seen open-source emulators in training and may reproduce their structure.
   - The review step checks for this, and `.claude/settings.json` blocks reads of
     `gba-eval/candidates`, `gba-eval/third_party`, `mesen_step.cpp` and web access.
   - The block only applies to Claude Code agents launched from this repo. Other agent tools need
     the same rule in their own config.
   - Shell commands that don't name those folders directly can still reach them. Treat the block as a
     guard rail, not proof.
8. **Overfitting.** An agent may "fix" a test by special-casing it. `CLAUDE.md` forbids branching on ROM
   identity, and the review step checks for it.
9. **Endstate tests are all-or-nothing.** One wrong pixel scores ~0. A test that "looks right" can
   still score 0. Compare `.cand.png` with `.ref.png` pixel by pixel.
10. **Fuel limit.** A slow PPU (per-pixel work with lots of branching) can trap on heavy games.
    Measure in Phase 6, or earlier if a replay "skips".
11. **Score ceiling.** The maximum is 0.96, not 1.0 (the `comprehensive` subsystem has no tests). Tell the
    client so expectations are set correctly.
12. **Grader runtime.** A full grade takes about 10 minutes and uses all CPU cores. Running two at once
    slows both. Set `RAYON_NUM_THREADS` if you must.
