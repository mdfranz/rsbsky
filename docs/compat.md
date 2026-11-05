# Compatibility with bskyban (Go)

**Baseline:** `../bskyban` commit `738e5e6` ("Fix staticcheck ST1023 in follows test"), built from
`git archive` of that commit. The working tree had an uncommitted change to `internal/tui/ui.go`
(textarea composer, `R` hint); it is TUI-only and not part of the Phase 0-1 baseline.

## Contract
Preserved: environment variables (`BSKY_HANDLE`, `BSKY_APP_PASSWORD`, `BSKY_PDS`, `BSKYBAN_STATE`,
`BSKYBAN_AUDIT`, `XDG_STATE_HOME` if absolute, `BSKYBAN_FANCY`, `BSKYBAN_THEME`, `BROWSER`, `NO_COLOR`),
command names, flag names/defaults/bounds, exit codes (0/1/2), `state.json` / `audit.jsonl` formats,
stdout of `feed --list [--json]`, and the request sequence sent to the PDS.

Deliberate differences (idiomatic clap instead of Go's `flag`):

| Area | Go | rsbsky |
|---|---|---|
| Help / usage text | `UsageText` | clap-generated `--help`; usage errors print `error: ...` plus a `--help` hint (exit 2) |
| Flag syntax | `-list`, `--list=false`, `-pages=0x10` | `--list`, plain switches, decimal numbers; negative `--pages` rejected |
| Durations | Go `ParseDuration` (`1.5m`) | `humantime` (`90s`, `1m30s`); fractions unsupported; bare `0` means off |
| Unknown command | logs in, then usage error (2) | usage error (2) without logging in; without credentials Go exits 1, rsbsky 2 |
| Empty first argument | Go panics (exit 2) | clap usage error (2) |
| JSON/network error wording | Go `encoding/json` / `net/http` text | serde / reqwest text (status and `error:` prefix match) |
| Duplicate-key / case-insensitive JSON keys | `Entries` matches `entries` | exact keys |

## Executable matrix
`scripts/compat.py` runs cases from `scripts/compat_cases.py` (isolated HOME/state/audit, scripted fake PDS,
pipe and PTY runs) and compares stdout, stderr, exit status, request trace and files left on disk.

```
python3 scripts/compat.py capture --go-bin ./bskyban   # refresh tests/compat/golden/ from the Go baseline
python3 scripts/compat.py check   --bin target/debug/rsbsky
python3 scripts/compat.py diff    --go-bin ./bskyban --bin target/debug/rsbsky   # live, no golden files
```
`cargo test` runs `check` (needs `python3`, not Go). Ages use `{AGO:...}` placeholders so output is stable;
`cmp: "status"` cases compare only the exit status because wording differs by design.

## Status by phase
| Phase | State |
|---|---|
| 0 Scaffold, CLI/env, paths, theme-name validation | done; every dispatch/validation path in the matrix passes |
| 1 Client, state reading, `feed --list [--json]` | done; matrix + wiremock tests pass, no mutations or state writes |
| 2 Feed TUI, `themes`, refresh, audit writer | not started |

## Temporary stubs (exit 1, `error: <cmd>: not yet implemented in rsbsky (planned: phase N)`)
Credentials, state loading and login run first, as in Go, so these fail after a successful login.
`feed`/`--json` TUI (2), `themes` (2, credential-free), `review` (5), `posts` incl. `--list/--json` (6),
`log` (4, credential-free), `list`, `baseline`, `show`, `block|unblock|follow|unfollow|keep` (4),
`post|delete-post|like|unlike` (6). Deferred matrix cases: `log_empty`, `themes_preview`,
`feed_tui_stub`, `list_json_without_list_is_tui`, `block_command`.
