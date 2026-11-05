# gba-emu: rules for every agent

Read PLAN.md before starting. It defines the phases, the shared contracts and who owns which file.
The benchmark lives at `/var/www/html/test-projects/gba-eval`, referred to below as `$GBA_EVAL`.
Use that absolute path; relative paths break inside worktrees.

## Originality (hard rule)
- Write all code yourself. Do not read, copy, or paraphrase any existing emulator:
  `$GBA_EVAL/candidates/` (gba-core), `$GBA_EVAL/third_party/` (Mesen2),
  `$GBA_EVAL/reference/mesen_step.cpp`, or mGBA/NanoBoyAdvance/VBA/etc. from memory or the web.
- Allowed sources: `$GBA_EVAL/spec/gbatek.htm` (GBA, ARM and THUMB chapters only), `$GBA_EVAL/spec/ABI.md`,
  `$GBA_EVAL/spec/gba_bios_stub.h`, the grader's outputs (scores, `.ref.png` vs `.cand.png`),
  and running ROMs from `$GBA_EVAL/corpus/roms/`.
- No per-ROM hacks: never branch on a ROM's hash, title, or game code. Fix the hardware behavior.

## Code readability
- Use clear, descriptive names for functions, variables and types: `destination_register`, `shift_amount`,
  `fn execute_data_processing`. Never cryptic ones like `x`, `t`, `rd2` or `tmp`. Loop indices
  (`i`, `line`) are fine when the meaning is obvious.
- Official gbatek register and field names are allowed (`DISPCNT`, `BG0CNT`, `IE`, `IF`, `CPSR`, `Rd`, `Rn`),
  each with a short plain-English comment where it is defined, e.g. `const DISPCNT: u32 = 0x000; // display control`.
- Every non-obvious function gets a one-line doc comment naming the gbatek section it implements, e.g.
  `/// Barrel shifter for data-processing operand 2 (gbatek: ARM Opcodes: Data Processing (ALU)).`
- Prefer small readable functions over clever one-liners. Split a long `match` arm or a dense bit-twiddling
  expression into a named helper.

## Ownership
- Only edit the files your task owns (see the ownership tables in PLAN.md).
- Shared files (`src/lib.rs`, `src/gba.rs`, `src/bus.rs`, `src/io.rs`, `src/irq.rs`, `src/scheduler.rs`,
  `Cargo.toml`) are changed only by the integrator (the executor session). If you need a contract
  change, say so in your report instead of making it.

## Git
- Remote: https://github.com/Rohan-Jalil/gba-eval-trial-rohan. Never push; the human pushes.
- Author and committer: Rohan Jalil <rohan.jalil@gmail.com>. This is set in this repo's git config;
  do not override it.
- Use Conventional Commits: `type(scope): summary`, for example `feat(cpu): add ARM data-processing ops`,
  `fix(ppu): wrap affine BG coordinates`, `test(timer): cascade overflow`, `docs(notes): phase 1 decisions`,
  `chore(results): phase 1 milestone`. Scopes: cpu, arm, thumb, bus, io, irq, ppu, apu, dma, timer,
  keypad, cart, abi, scripts, plan, notes, results.
- Commit after every meaningful step (a working instruction group, a passing test, a merged branch),
  not one large commit per phase.
- Worker agents commit on their own branch. Only the integrator merges into `main` (`git merge --no-ff`).

## Logging
- `NOTES.md`: add an entry for every decision or change of approach (date, what, why), and for every
  phase record which agents ran and on which model. The integrator writes the agent/model rows.
- `TIMELOG.md` belongs to the human. Agents never edit it.
- Milestones: `scripts/grade-full.sh <phase> "<note>"` saves `results/milestones/<phase>/`. Commit that
  folder and `scores.tsv`. All other `results/` output stays ignored.

## Before you report done
1. `cargo test` passes.
2. `cargo build --release --lib --target wasm32-unknown-unknown` succeeds.
3. `scripts/grade-subset.sh target/wasm32-unknown-unknown/release/gba_emu.wasm <name> <your tests> <smoke set>`.
   Report the per-test scores, and never lower a smoke-set score without saying so.
4. Your work is committed on your branch, and your report lists the commits.
