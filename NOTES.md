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
| 2026-11-05 | IO owners take `io_write16(offset, value, mask)` (mask 0x00FF/0xFF00/0xFFFF); reads stay `io_read16(offset)`. Contract change from PLAN.md, accepted by the integrator | Turning an 8-bit write into a 16-bit read-modify-write breaks IF acknowledge, HALTCNT vs POSTFLG, the sound FIFOs and write-only registers. The BIOS stub's halt is itself a byte write. |
| 2026-11-05 | `Bus` owns every component except the CPU; the PPU owns palette/VRAM/OAM; DMA is a free function over `&mut Bus`; access costs accumulate in `bus.access_cycles`, drained by the CPU | The CPU only sees the bus, gba.rs can borrow sibling fields without reference cycles, and `read32(addr) -> u32` stays as planned. |
| 2026-11-05 | ROM buffer, framebuffer and audio buffer live outside `Gba` (framebuffer copied once per frame) | ABI pointers stay stable across load and reset. |
| 2026-11-05 | Frame boundary: `gba::FRAME_END_LINE = 0`. Each `emu_run_frame` runs lines 0..227 (the picture, then all of VBlank), 280,896 cycles, starting at line 0 on power-on | Measured against Mesen2 with grader output only: a dummy wasm graded on custom testcases (tonc key-demo, A pressed from frame P, final `.ref.png` compared losslessly). Pressed at frame 33, it shows at frame 34; pressed at 34, it does not show at 34. key-demo polls keys after waiting for VBlank, so the reference frame must contain the VBlank that follows the picture. Ending at line 160 would give 0-frame latency. |
| 2026-11-05 | Boot alignment is still open; it becomes a Phase 1 integrator check | The grader burns 6 reference "boot frames" (Mesen SkipBootScreen warmup) and 0 for us. tonc m3-demo reference frame 0: rows 0-36 drawn, row 37 has 131 of 240 px, rows 38+ black; frame 1 is full. So the game is still drawing when the beam overtakes it, and probably started before frame 0. Once the CPU runs, compare our m3-demo frame 0 with that. If we are behind, run extra cycles inside `emu_load_rom`/`emu_reset` (allowed by `fuel_load_rom`), not `emu_boot_frames`. |
| 2026-11-05 | Phase 0 review follow-ups deferred (integrator-owned, shared files): SRAM 16/32-bit access should use the exact address and rotate write values; ignore 8-bit VRAM writes to the OBJ area; mirror 0x04000800 every 64K; re-check the VCOUNT match flag on DISPSTAT writes and at power-on; readability of the BIOS read fold (bus.rs) and the HALTCNT mask (irq.rs) | Reviewer rated these minor and not needed for the Phase 0 gate. Bus items go with the Phase 2 memory work; the DISPSTAT one goes to the Phase 1 ppu-basic worker. |

## Agent and model usage per phase

| Date | Phase | Agent (role) | Model | Branch | Outcome |
|---|---|---|---|---|---|
| 2026-11-05 | planning | main session (planning, scripts) | claude-opus-5-5 | main | PLAN.md, CLAUDE.md, scripts, agent definitions, executor prompts |
| 2026-11-05 | 0 | executor (integrator, frame-boundary probe, merge) | claude-opus-5-5 | main | Preflight OK; FRAME_END_LINE set to 0 from the probe |
| 2026-11-05 | 0 | skeleton-builder | claude-opus-5-5 | phase0/skeleton | Merged. 6 cargo tests pass, wasm has no imports, smoke set and gate run with no trap (all ~0, as expected with a stub CPU) |
| 2026-11-05 | 0 | reviewer | claude-opus-5-5 | phase0/skeleton | 1 round: verdict merge, originality clean; 1 major (frame boundary, fixed by the integrator), 6 minor deferred |
