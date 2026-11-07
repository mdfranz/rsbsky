# rsbsky

A Rust/Ratatui port of [bskyban](../bskyban): review Bluesky followers, browse your timeline and moderate.
Built incrementally (see `plan.md`); same environment variables and state/audit files as the Go tool.

Working today (Phases 0-4): `rsbsky` opens the Following timeline (`↑ ↓ PgUp PgDn Home End`, `→` post detail
with profile stats and recent posts, `Enter`/`o` link picker, `i` images, `R` refresh, `b f k` block/follow/keep, `u` unfollow, `U` undo, `F` who they follow, `q` quit; `--refresh 30s`, `--fancy`, `--theme`), `rsbsky themes` previews themes, and
`rsbsky feed --list [--json] [--pages N] [--backlog]` prints the repost authors the Reposts tab will offer.
Headless: `log`, `list`, `baseline`, `show`, `block|unblock|follow|unfollow|keep [--dry-run]`.
Everything else is a documented stub: see `docs/compat.md`.

```
export BSKY_HANDLE=you.bsky.social BSKY_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx
cargo run -- feed --list --backlog
```
Checks: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
# rsbsky

## Optional observability (Logfire)

Set `LOGFIRE_TOKEN` (a [Logfire write token](https://logfire.pydantic.dev/docs/how-to-guides/create-write-tokens/))
and rsbsky exports OpenTelemetry traces to [Pydantic Logfire](https://logfire.pydantic.dev). Without the variable
(or with it empty) nothing is configured, no connection is made and behaviour is identical.

```
LOGFIRE_TOKEN=... LOGFIRE_ENVIRONMENT=dev rsbsky feed
```

* **Sent:** the command name, one span per API call (operation such as `app.bsky.feed.getTimeline`, HTTP status,
  outcome, XRPC error *code*), TUI background work (timeline pages, refresh checks with the number of new posts,
  moderation actions by kind and outcome), exit code, durations, `service.name=rsbsky`, `service.version`.
* **Sent only when you open a link:** the full URL of each link you open from the TUI, as `url.full`, with how it was
  opened and whether it worked. These can contain handles and DIDs, and together they are a record of what you read.
  The same links are always kept locally in the audit log, with or without Logfire: `rsbsky log --action open`.
* **Never sent otherwise:** the app password, access tokens, API request URLs, hostnames or IP addresses, request
  bodies, handles, DIDs, post text or error messages. Dependencies' own debug events (HTTP connection pooling, TLS) are filtered out by
  capping the level at `INFO`; `RUST_LOG` overrides that when you want to debug.
  The tests inspect the exact exported bytes for this (`make telemetry-check`).
* **Never on screen:** console output is disabled, so the TUI is not drawn over; an unreachable collector does not
  change output or exit status, and exit waits at most 3 seconds for the final flush.
* The usual Logfire variables work: `LOGFIRE_SERVICE_NAME`, `LOGFIRE_ENVIRONMENT`, `LOGFIRE_BASE_URL`,
  `LOGFIRE_SEND_TO_LOGFIRE=no` (turns it off even with a token), `RUST_LOG`.
* Build without it with `cargo build --no-default-features` (about 3 MB smaller, 42 fewer crates).
