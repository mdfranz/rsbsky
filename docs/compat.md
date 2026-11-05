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
| 3 Post detail view, link picker, image/link openers | done; see "Phase 3 notes" |
| 4 State writing, audit reading, moderation actions | done; see "Phase 4 notes" |

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

## Phase 3 notes
`→` opens the detail view for the selected post (profile panel left, post panel right; stacked below 96 columns),
`← →` / `↑ ↓` step through posts, `PgUp PgDn` scroll the post panel, `Esc`/`Backspace` return. The profile panel
shows account age, joined date, avatar, posts, followers, following, posts/day, follower ratio and "follows you"; the
post panel shows reply context, text, attachments, link card, quote, image links, engagement counts and the author's
five most recent posts (`getAuthorFeed`, `posts_with_replies`). `Enter`/`o` opens the link picker (post, card, URLs in
the text, quoted post, mentions, author profile) and `i` opens every image; both launch `$BROWSER` (first of its
colon-separated commands, `%s` = URL) or `open` / `xdg-open`, with the launcher's stdio detached from the TUI.

- **Hyperlinks inside wrapped text:** `termtext::wrap_linked` returns link spans per fragment (a URL split over
  several lines links every fragment to the full URL); the same post-draw pass lays them over the frame.
  `wrap`, `find_urls` and the mention finder are differentially tested against Go on 396 cases
  (`tests/compat/wrap_cases.json`, generated by running Go's `termtext`).
- **While a post is open, new posts are held** (`Following (+N new)`); `R` closes the post, inserts them and jumps
  to the top. Profile and recent posts are prefetched for the current and next author and retried after an error.
- **Differences from Go:** the engagement line moves "open post ↗" to its own row instead of running off a narrow
  panel; a link clipped by the panel edge stays a link over the visible cells; the hint bar lists only implemented
  keys; account age is computed from an injected clock (testable) rather than the wall clock at render time.

## Phase 4 notes
**Headless:** `log` (credential-free), `list`, `baseline`, `show`, and `block | unblock | follow | unfollow | keep`
with `--dry-run` and `-` (identifiers from stdin) are byte-for-byte compatible with Go on stdout, exit status, the
request sequence *including bodies*, and the resulting `state.json` / `audit.jsonl` (parsed equality; timestamps
written "just now" are normalised) and file modes (0700 directories, 0600 files). `state.json` is written atomically
(tmp + rename) and rolled back in memory when the save fails; unknown fields and `null` entries survive a rewrite.

**Feed keys:** `b` block (asks again when you follow them), `f` follow, `k` keep, `u` unfollow (asks again), `U` undo,
`F` browse who the author follows (`Enter`/`o` opens a profile, `q`/`Esc` closes, `R` retries). Decisions are audited as
`source: tui:following` with `via` (who reposted the author) and a profile snapshot (counts only if the full profile
had loaded); overriding a block/follow audits the lifting `unblock`/`unfollow` first.

**Remote result and local save are separate.** If the API call succeeds but the state file cannot be written, the
result is kept as *pending*: the footer says so, the failure is audited (`action completed; local persistence failed`),
every other key and `q` are refused, and `r` retries *only* the save (no second mutation) until it succeeds. Quitting
while an action is running waits for it, saves and audits, then exits; a failed action cancels the quit and shows the
error. Undo restores the previous decision *and* its remote relationship; restoring a lifted block/follow creates a
new record, and the earlier history entry is updated so a later undo deletes the replacement.

**Verification:** `scripts/compat.py` (172+ cases against Go), unit tests for every key path including failure
injection, wiremock tests of the remote half (`tests/actions.rs`), tmux checks (decisions, confirmation, failure,
read-only state directory with `r` recovery, quit during a slow action), and `scripts/roundtrip.py --go-bin G --bin R`
(sequential Go->Rust->Go writes over shared files; needs the Go binary, so it is not part of `cargo test`).

**Differences from Go:** usage errors (`block` with no identifiers, bad flags) are reported before logging in;
OS error wording in save failures differs ("Is a directory (os error 21)"); `log --since` takes `humantime`
durations plus the `Nd` day form (no fractional hours).

## Temporary stubs (exit 1, `error: <cmd>: not yet implemented in rsbsky (planned: phase N)`)
Credentials, state loading and login run first, as in Go, so these fail after a successful login.
`review` (5), `posts` incl. `--list/--json` (6), `post|delete-post|like|unlike` (6). The Reposts tab (5) and
reply / like / compose / delete keys (6) are not bound yet. Deferred matrix case: `themes_preview_tty`.
