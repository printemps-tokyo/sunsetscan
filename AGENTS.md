# AGENTS.md

Project instructions for AI coding agents working in this repository.

## What this is

sunsetscan is an offline Rust CLI that scans a codebase for calls into APIs with
an announced shutdown date and reports each hit with `file:line`, the days
remaining and the official migration path. No network access at runtime, ever.

## Layout

| Path | Role |
| --- | --- |
| `src/catalog.json` | The catalog data, embedded at build time. Adding an entry is a one-file change. |
| `src/catalog.rs` | Catalog types, JSON validation, merge and filtering. |
| `src/matcher.rs` | Pure matching: catalog plus text in, hits out. Boundary semantics live here. |
| `src/model.rs` | Pure rollup: hits in, a grouped and ordered report out. Urgency and exit-code policy. |
| `src/report.rs` | Pure rendering: text, `--list`, JSON. |
| `src/scan.rs` | The only module that touches the filesystem. |
| `src/date.rs`, `src/json.rs` | Local calendar and JSON, deliberately not crates. |
| `src/main.rs` | Argument parsing and wiring only. |

Keep the split. Logic that can be tested from in-memory inputs belongs in the
library; the binary does args, walking and writing to stdout.

## Non-negotiables

- **Never encode a sunset date that was not read on a vendor-owned page.**
  Every entry carries `source_url` and `verified_on`. If a date cannot be
  confirmed, leave the entry out. A wrong date is worse than a missing one.
- **Never invent SDK symbol names.** Patterns come from what the vendor's docs
  and migration guides actually print.
- **Do not add language parsing.** Matching is textual by design and the README
  says so plainly. Changing that is a project-level decision, not a refactor.
- **Do not add dependencies** beyond `anyhow`, `clap` and `ignore` without
  justifying why a few dozen lines of local code is worse.
- No emoji anywhere: code, comments, docs, commit messages.
- All deliverables in English.

## Conventions

- Output must be deterministic: fixed wrap width, sorted everywhere, no
  terminal-derived formatting, no locale dependence.
- Every test pins `--as-of`, so nothing depends on the day it runs.
- Conventional Commits, English, no `Co-Authored-By` trailer.

## Before you finish

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo build --release
./target/release/sunsetscan . --strict --no-color --as-of 2026-08-11
```

CI runs a newer stable toolchain than the local one, with stricter clippy
lints. Write modern idiomatic Rust (inline format args, `let ... else`,
`is_some_and`) so it stays clean there.
