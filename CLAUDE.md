# toppingctl

Cross-platform (macOS + Linux) Rust TUI for the TOPPING E2x2 USB audio interface
(USB `152a:8752`), controlling it over its HID interface. Single-repo project.
**Start singularity sessions here** so this index loads as context.

## Repos (what this project spans)
- toppingctl (this repo) — device library (HID codec, I/O, typed state), CLI, ratatui TUI,
  workspaces, packaging. Issues for this project live here (`ktoulgaridis/toppingctl#N`).

## Project rules
- **Clean-room.** Protocol facts come only from our own captures and our own analysis of the
  official app. Other projects (tpmix, adryzz/nukoyaki toppingctl, E2x2-Switch) are
  *unverified hints* — never copy their code, never run their binaries.
- **Never commit** TOPPING's app binary, Ghidra projects or anything extracted from them.
  The RE workspace lives outside this repo. Only `PROTOCOL.md` and our own HID captures
  (used as test fixtures) are committed.
- **No hardware in CI.** Tests run against recorded captures; on-device checks are manual
  acceptance steps.
- **48V phantom power** is never turned on, restored or loaded from a workspace without an
  explicit confirmation.

## Operating model
Follows the Kyriakos operating model — run /singularity:prime to load it from
singularity-wiki. Project *structure* lives here; org *process/knowledge* lives in the wiki.
