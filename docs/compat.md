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
| 2 Feed TUI (Following), `themes`, refresh, audit writer | done; see "Phase 2 notes" |

## Phase 2 notes
Implemented: `rsbsky` / `rsbsky feed` Following view (list with up to 3 text lines, quote, repost/reply tags,
age as an OSC 8 link, key-hint bar, load-more near the end), keys `↑ ↓ PgUp PgDn Home End R q Ctrl+C`,
background refresh (`--refresh`, up to 4 pages, overlap/gap logic, hold with `(+N new)`), `--fancy` /
`BSKYBAN_FANCY`, `--theme` / `BSKYBAN_THEME` / `NO_COLOR`, `themes`, and the audit writer for refresh failures.

- **Hyperlinks:** link targets stay out of ratatui's cell buffer; a post-draw pass (`src/tui/links.rs`) re-prints
  the link cells wrapped in OSC 8 every frame and re-prints cells that stopped being links as plain text.
  Off when `TERM` is `dumb` or `linux`.
- **No terminal queries:** startup never sends `ESC[6n`; a terminal that does not answer cannot stall it.
- **Differences from Go:** the feed has no Reposts tab yet (Phase 5) so the title row shows only `Following`;
  the hint bar lists only keys that work today; adaptive colours guess the background from `COLORFGBG`
  (default dark) instead of asking the terminal; a failed manual `R` clears the "checking…" status;
  without a terminal `feed` exits 1 after login with its own wording (Go: "could not open a new TTY").
- **Checks:** unit tests (state machine, layout snapshots, link bytes), wiremock tests for the check loop,
  `scripts/tui_check.py` (tmux + raw pty: restore after quit/Ctrl+C/SIGTERM/panic/slow HTTP, resize,
  NO_COLOR, refresh insert/hold/`R`, audit lines read back by Go's `log`). Run the slow suite with
  `cargo test -- --ignored`.

## Temporary stubs (exit 1, `error: <cmd>: not yet implemented in rsbsky (planned: phase N)`)
Credentials, state loading and login run first, as in Go, so these fail after a successful login.
`review` (5), `posts` incl. `--list/--json` (6), `log` (4, credential-free), `list`, `baseline`, `show`,
`block|unblock|follow|unfollow|keep` (4), `post|delete-post|like|unlike` (6). Detail view, link picker,
`o`/`i` openers (3) and all moderation keys (4) are not bound yet. Deferred matrix cases: `log_empty`,
`block_command`, `themes_preview_tty`.
