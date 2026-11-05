# 3rd Party Dependencies

This document lists and classifies the crates `rsbsky` depends on (see `Cargo.toml`; `Cargo.lock` pins the full tree). Bluesky is reached with `reqwest` and `serde_json` against the AT Protocol XRPC endpoints, with no Bluesky SDK, the same approach as the Go original (`../bskyban`). The edition is 2024 and the `logfire` crate needs Rust 1.91, so `rust-version = "1.91"` (developed on 1.96).

## Terminal UI

*   **[ratatui](https://github.com/ratatui/ratatui)** (0.30): TUI rendering. `src/tui/render.rs` draws the feed, the two-panel detail view, the picker and the follows browser into a `Buffer`; tests render into a plain `Buffer` and compare rows as text. rsbsky does **not** use ratatui's widgets or layout engine: it writes lines with `Buffer::set_line` / `set_style` so the layout matches the Go original cell for cell.
*   **[crossterm](https://github.com/crossterm-rs/crossterm)** (0.29, `event-stream`): raw mode, the alternate screen, cursor control and key events. `src/tui/runtime.rs` reads `EventStream` inside a `tokio::select!` loop, and `TerminalGuard` restores the terminal on exit, error and panic. `cli.rs` also uses `terminal::size()` to truncate `feed --list` output to the terminal width.
*   **[unicode-width](https://github.com/unicode-rs/unicode-width)**: display width of characters and strings (CJK, emoji, combining marks). Used by `src/termtext.rs` for wrapping and truncation and by the renderer to place hyperlinks on the right cells.
*   **[regex](https://github.com/rust-lang/regex)**: URL, bare-domain and `@mention` detection in post text (`termtext::find_links`, `wrap_linked`, `find_mentions`). The patterns are ports of the Go ones and are tested against Go's output.

## Async and HTTP

*   **[tokio](https://tokio.rs)** (`rt-multi-thread`, `macros`, `time`, `signal`, `sync`): the runtime. The TUI event loop, the refresh and animation timers, `SIGTERM`/`SIGHUP` handling and the background requests (`JoinSet`, aborted on quit) all run on it. The headless commands run on the same `#[tokio::main]`.
*   **[futures-util](https://github.com/rust-lang/futures-rs)**: `StreamExt::next` on crossterm's `EventStream`.
*   **[reqwest](https://github.com/seanmonstar/reqwest)** (0.13, `default-features = false`, `rustls`): the HTTP client behind `bsky::Client`, with a 30 second timeout. TLS is rustls, so there is no OpenSSL dependency; the JSON, query and form helpers are not enabled because the client builds URLs and bodies itself.

## Data and errors

*   **[serde](https://serde.rs)** and **[serde_json](https://github.com/serde-rs/json)**: every XRPC response, `state.json` and `audit.jsonl`. Structs use `#[serde(default)]` plus a small `nz` helper so JSON `null` behaves like a missing field, as in Go's decoder.
*   **[chrono](https://github.com/chronotope/chrono)**: RFC 3339 parsing for post and account ages and for the timestamps stored in `state.json`; `store::state::go_time` reproduces Go's `RFC3339Nano` formatting.
*   **[thiserror](https://github.com/dtolnay/thiserror)**: the `bsky::Error`, `StateError` and `UnknownTheme` types. Application-level failures are plain strings (`CliError`) because they are printed, never matched on.

## Command line

*   **[clap](https://github.com/clap-rs/clap)** (4, `derive`, `env`): subcommands, flags, `--help` and usage errors (exit code 2). Flag syntax follows clap conventions rather than Go's `flag` package; the environment variables are read by hand so their semantics (empty means unset, `XDG_STATE_HOME` only if absolute) match the Go tool exactly. See `docs/compat.md`.
*   **[humantime](https://github.com/chronotope/humantime)**: duration syntax for `--refresh` (`30s`, `1m30s`) and `log --since`.

## Observability (optional)

*   **[tracing](https://github.com/tokio-rs/tracing)**: the instrumentation API. Commands, API requests and the TUI's background tasks are wrapped in spans (`src/cli.rs`, `src/bsky/client.rs`, `src/tui/runtime.rs`). With no subscriber installed these are close to free, so the code is instrumented unconditionally.
*   **[logfire](https://github.com/pydantic/logfire-rust)** (0.13, optional `logfire` feature, on by default): the Pydantic Logfire SDK, an OpenTelemetry wrapper that installs a `tracing` subscriber and exports over OTLP/HTTP from its own background thread. `src/telemetry.rs` configures it **only when `LOGFIRE_TOKEN` is non-empty**, with the console exporter disabled. It brings OpenTelemetry (`opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`, ...) and shares our `reqwest`/`rustls`. Measured cost: +2.9 MB in the release binary (11.3 -> 14.2 MB) and +42 crates; `--no-default-features` removes all of it.

## Testing (dev-dependencies)

*   **[wiremock](https://github.com/LukeMathWalker/wiremock-rs)**: a fake PDS for `tests/client.rs` and `tests/actions.rs`. It asserts the exact requests (paths, query, bodies, bearer token) and that read-only paths never mutate.
*   **[tempfile](https://github.com/Stebalien/tempfile)**: isolated state and audit files in unit tests, including failure injection (a directory where the temp file should go).

## Test tooling that is not a crate

*   **Python 3** (standard library only) runs `scripts/telemetry_check.py` (a fake OTLP collector), `scripts/compat.py` (the executable compatibility matrix and its fake PDS), `scripts/tui_check.py` and `scripts/roundtrip.py`. `cargo test` runs the matrix and the fast terminal-bytes check; the slow suite is `make tui-check`.
*   **tmux** drives the real-terminal suite (screen contents, alternate screen, cursor visibility, tty modes). Optional: those tests are skipped or `#[ignore]`d without it.
*   **The Go `bskyban` binary** is the oracle for `make compat-capture`, `compat-diff` and `roundtrip`. It is not needed to run `cargo test`: the golden outputs it produced are checked in under `tests/compat/golden/`.

## Not dependencies

*   **No Bluesky/AT Protocol SDK.** `src/bsky` calls `com.atproto.*` and `app.bsky.*` XRPC methods directly.
*   **No database.** Decisions live in `state.json` and the audit log is `audit.jsonl`, both via `serde_json` and `std::fs`, in the same formats as the Go tool so the two can share the files.
*   **No browser or image library.** Links and images open through `$BROWSER`, else `open` (macOS) or `xdg-open`, via `std::process::Command` with its stdio detached from the TUI.
*   **No Unicode segmentation crate yet.** The 300-grapheme post limit arrives with the composer (Phase 6), which is when `unicode-segmentation` will be added.
*   **No terminal-capability crate.** Hyperlinks are written as plain OSC 8 sequences (`src/tui/links.rs`), disabled only when `TERM` is `dumb` or `linux`.
