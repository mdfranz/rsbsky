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
