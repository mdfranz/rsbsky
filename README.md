# rsbsky

A Rust/Ratatui port of [bskyban](../bskyban): review Bluesky followers, browse your timeline and moderate.
Built incrementally (see `plan.md`); same environment variables and state/audit files as the Go tool.

Working today (Phases 0-2): `rsbsky` opens the Following timeline (`↑ ↓ PgUp PgDn Home End`, `R` refresh,
`q` quit; `--refresh 30s`, `--fancy`, `--theme`), `rsbsky themes` previews themes, and
`rsbsky feed --list [--json] [--pages N] [--backlog]` prints the repost authors the Reposts tab will offer.
Everything else is a documented stub: see `docs/compat.md`.

```
export BSKY_HANDLE=you.bsky.social BSKY_APP_PASSWORD=xxxx-xxxx-xxxx-xxxx
cargo run -- feed --list --backlog
```
Checks: `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`.
