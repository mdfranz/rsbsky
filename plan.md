# rsbsky: Rust/Ratatui port of bskyban

## Context
`../bskyban` (Go, Bubble Tea, ~8.2k LOC) is a Bluesky TUI for reviewing followers, browsing the home timeline, and moderating (block/follow/keep). It uses plain XRPC REST polling with an app password. There is no firehose, DB, or config file. `rsbsky` is an empty git repo. Goal: a behavior-compatible Rust port, built incrementally, **starting with the feed view**, keeping the **CLI flags and environment variables identical** so it works as a drop-in replacement (same state/audit files too).

## Stack
`ratatui` + `crossterm`, `tokio`, `reqwest` (rustls, 30s timeout), `serde`/`serde_json`, `clap` (derive; manual command dispatch semantics preserved, see Phase 0), `unicode-segmentation` + `unicode-width`, `anyhow`/`thiserror`, `directories`-free XDG logic (hand-rolled to match Go). Tests: `wiremock` (fake PDS, equivalent to `internal/testpds`), `insta` or ratatui `TestBackend` for render snapshots.

## Layout
```
src/main.rs        dispatch, exit codes (0 ok, 1 err, 2 usage)
src/cli.rs         flags, UsageText, env resolution
src/bsky/          client.rs (XRPC, createSession), feed.rs, post.rs, profile.rs, records.rs
src/store/         state.rs (state.json), audit.rs (audit.jsonl)
src/app/           is_new, feed_candidates, create_for/delete_for
src/tui/           app.rs (event loop), feed.rs, theme.rs, widgets (osc8, wrap), later: review, picker, follows, compose
src/termtext.rs    sanitize (strip ANSI/control), OSC 8, grapheme wrap
tests/             wiremock-based integration tests
```

## Compatibility contract (fixed in Phase 0, honored throughout)
- Env: `BSKY_HANDLE`, `BSKY_APP_PASSWORD` (required except `log`/`themes`/`help`), `BSKY_PDS` (default `https://bsky.social`), `BSKYBAN_STATE`, `BSKYBAN_AUDIT`, `XDG_STATE_HOME` (only if absolute; else `~/.local/state/bskyban`), `BSKYBAN_FANCY` (1/true/yes, ORed with `--fancy`), `BSKYBAN_THEME` (default `auto`, `--theme` overrides), `BROWSER` (else `open`/`xdg-open`), `NO_COLOR`.
- Commands: `feed` (default when no command or first arg starts with `-`), `review`, `posts`, `list`, `log`, `themes`, `baseline`, `show`, `block|unblock|follow|unfollow [--dry-run] (ids|-)`, `keep`, `post`, `delete-post`, `like|unlike`, `help|-h|--help`.
- Feed flags: `--pages N` (3), `--backlog`, `--refresh DUR` (30s; 0=off; else >=10s or usage error exit 2), `--fancy`, `--theme`, `--list`, `--json` (JSON Lines: did, handle, displayName, repostedBy, text).
- Flags are parsed before login, so usage errors need no creds/network. Unknown theme → exit 2. Reuse `UsageText` verbatim from `bskyban/internal/cli/cli.go:26-58`. Likely use clap only for flag parsing with a custom top-level dispatcher to match Go's `flag` behavior; verify against the Go binary.
- Files: `state.json` (`{"entries":{did:{handle,decision,at}}}`, atomic tmp+rename, 0700/0600) and `audit.jsonl` (fields per `store/audit.go`) must be byte-compatible so both tools can share them.

## Phases

### Phase 0: Scaffold & CLI/env compatibility
- `cargo init`, deps, CI (fmt, clippy, test).
- Implement `cli.rs` (dispatch, all flag definitions even for unimplemented commands, which print "not yet implemented" with exit 1), env/XDG path resolution, theme-name validation, `help`/usage text.
- Tests: table-driven parsing tests mirroring Go's `cmd/bskyban` tests (refresh<10s, unknown theme, no-command → feed, `--` handling).
- Exit: `rsbsky --help`, bad flags, and path resolution behave like `bskyban`.

### Phase 1: API client + data model
- `createSession` with handle/app password → `accessJwt` Bearer (no refresh, matching Go). Error format `nsid: message (error)`.
- `getTimeline` (limit=100, cursor), `getProfile(s)`. Post model parsing embeds: images, video, external, record, recordWithMedia; `reasonRepost`; reply parent.
- wiremock fake PDS fixtures; port Go `bsky` tests.
- Exit: `rsbsky feed --list [--json]` works headless against a real or fake PDS. Compare output to `bskyban feed --list --json` for the same account.

### Phase 2: Feed view TUI (first UI milestone)
- Alternate-screen ratatui app, async event loop (crossterm `EventStream` + tokio tick + HTTP result channel).
- Following list: up to 3 text lines, quoted post (2 lines, `▎`), `↻ reposted by @who · written 8m ago`, `↳ replying to @who` (indent for same-thread replies), age as OSC 8 link to bsky.app, link cards `↗ Title url`, image marker `▣`, key-hint bar, scrolling with cursor-pagination load-more near the end.
- Keys: ↑/↓, PgUp/PgDn, `R`, `q`/ctrl+c. Themes (`auto,dark,light,ansi,dracula,catppuccin`, `NO_COLOR`), `themes` preview command.
- Refresh: ticker per `--refresh`, silent check of up to 4 pages; first-run inserts immediately, otherwise header `Following (+N new)`, `R` jumps to top and inserts; failure shows `⚠ refresh failed`. Fancy mode: highlight bar + `new` tag fading over 2s with an 80ms tick.
- OSC 8 caveat: ratatui buffers cell-by-cell and mangles raw escapes. Needs a spike first (options: embed escapes in a cell's symbol with width fix-ups, or a post-draw pass). Do this spike at the start of the phase.
- Tests: `TestBackend` snapshots for layout/wrap/new-indicator, refresh logic unit tests.

### Phase 3: Post detail view
- `→`/`←`/`Esc`: two-panel layout (profile left, post right), counts, parent/quoted post, image links, author's recent posts (`getAuthorFeed posts_with_replies`), profile stats (age, posts/day, follower ratio, follows-you). Next/prev post navigation, panel scroll.
- Link picker (Enter/o) and `i` open images, using `BROWSER`/`open`/`xdg-open`.

### Phase 4: State, audit, moderation actions
- `store::state` + `store::audit`, `app::{is_new, create_for, delete_for}`.
- Feed actions: `b` (confirm if followed), `f`, `k`, `u` (confirm), `U` undo, `F` follows browser. Headless commands: `block/unblock/follow/unfollow/keep/baseline/show/log/list` incl. `--dry-run`, stdin `-`, audited with `source` = `cli` or `tui:*`.

### Phase 5: Reposts tab & Review
- Tab switching; Reposts tab (original authors, skipping followed/blocked/decided; refresh appends). `review` command (one follower at a time; `b f k s u U r q`, Home/End, handled-state badges, `--since`, `--backlog`).

### Phase 6: Writing posts & `posts` view
- Composer (300-grapheme counter, facets for links, reply root/parent threading, keeps text on failure), like/unlike `h`, delete own post `d`, `post`/`delete-post`/`like`/`unlike` CLI, `posts` command.

### Phase 7: Polish & parity
- Avatar/images (optional; Go uses browser open only), cross-tool parity script analogous to `scripts/smoke.sh` (read-only, `--dry-run`, temp dirs), README, release builds, side-by-side diff of `--list --json` and state/audit files.

## Reference files (Go)
`cmd/bskyban/main.go:27-124` (dispatch), `internal/cli/cli.go` (usage/flags), `internal/tui/ui.go` (feed, keys, refresh ~L240), `internal/tui/theme.go:34-70`, `internal/bsky/*`, `internal/store/*`, `internal/app/app.go:43-111`, `internal/termtext`, `internal/testpds`.

## Verification
- Per phase: `cargo fmt --check && cargo clippy -- -D warnings && cargo test`.
- Phase 1-2: run `rsbsky feed --list --json` vs `bskyban feed --list --json` with the same env and diff. Run the TUI against the real account (read-only) and the wiremock PDS; check render snapshots.
- Phase 4+: share one `BSKYBAN_STATE`/`BSKYBAN_AUDIT` between both binaries with `--dry-run` and confirm each can read the other's files.
- Manual TUI check in a real terminal (tmux) for OSC 8 links, resize, and NO_COLOR.
