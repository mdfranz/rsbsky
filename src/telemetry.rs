//! Optional observability through [Pydantic Logfire](https://logfire.pydantic.dev).
//!
//! rsbsky is instrumented with the `tracing` crate (spans for commands, requests and TUI
//! background work), which costs nothing while no subscriber is installed, plus histograms
//! of how long the TUI takes to respond (see [`Turn`]). Exporting is
//! switched on **only** when `LOGFIRE_TOKEN` is set and non-empty, and only if the crate was
//! built with the `logfire` feature (the default).
//!
//! What is sent: the command, operation names, HTTP method/status, XRPC error *codes*,
//! counts, outcomes and timings. What is never sent: the app password, access tokens,
//! request URLs or bodies, handles, DIDs, post text, keystrokes (a turn says only that it
//! was caused by a key), or error messages that could contain them. The console exporter is
//! disabled, so the terminal UI is never drawn over.
//!
//! The one exception is [`record_open`]: the links the user chooses to open are sent in full,
//! and those can contain handles and DIDs. API request URLs stay private.
//!
//! Standard Logfire environment variables apply (`LOGFIRE_SERVICE_NAME`,
//! `LOGFIRE_ENVIRONMENT`, `LOGFIRE_BASE_URL`, `LOGFIRE_SEND_TO_LOGFIRE`, `RUST_LOG`).

use std::time::Duration;

/// A turn at least this long also emits a `slow ui turn` event with its breakdown. Well below
/// the roughly 100 ms at which a response stops feeling instant.
const SLOW_TURN: Duration = Duration::from_millis(50);

/// One trip round the TUI loop: from waking up to the frame being written. For a key press
/// this is input-to-screen, minus any time the key sat queued behind an earlier turn.
#[derive(Debug, Default, Clone, Copy)]
pub struct Turn {
    /// What woke the loop. Always one of a fixed set of labels, never user data.
    pub cause: &'static str,
    /// Running the key or message handler.
    pub handle: Duration,
    /// Performing effects, which includes blocking work such as appending to the audit log.
    pub effects: Duration,
    /// Rendering and writing the frame.
    pub draw: Duration,
}

impl Turn {
    fn total(&self) -> Duration {
        self.handle + self.effects + self.draw
    }

    fn is_slow(&self) -> bool {
        self.total() >= SLOW_TURN
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Records a finished turn. Called for every frame, so it only touches histograms unless the
/// turn was slow: one span per frame would be noise at ten frames a second.
pub fn record_turn(turn: &Turn) {
    #[cfg(feature = "logfire")]
    if let Some(ui) = UI.get() {
        ui.record_turn(turn);
    }
    if turn.is_slow() {
        tracing::info!(
            cause = turn.cause,
            turn_ms = ms(turn.total()),
            handle_ms = ms(turn.handle),
            effects_ms = ms(turn.effects),
            draw_ms = ms(turn.draw),
            "slow ui turn"
        );
    }
}

/// Records how long a background task's result took to reach the screen, measured from
/// starting the task to writing the frame that shows it: the wait the user actually sees.
pub fn record_result_wait(task: &'static str, wait: Duration) {
    #[cfg(feature = "logfire")]
    if let Some(ui) = UI.get() {
        ui.result_wait
            .record(ms(wait), &[opentelemetry::KeyValue::new("task", task)]);
    }
    #[cfg(not(feature = "logfire"))]
    let _ = (task, wait);
}

/// Records a link the user chose to open, in full: the only URL rsbsky exports, and only ever
/// from this deliberate action. `via` is how it was opened; the launcher's error is not sent.
pub fn record_open(url: &str, via: &'static str, opened: bool) {
    tracing::info!(
        url.full = url,
        via,
        outcome = if opened { "ok" } else { "error" },
        "open link"
    );
}

/// Scale 4 keeps quantiles within about 5% and adapts its range to the data, so there are no
/// bucket boundaries to choose.
#[cfg(feature = "logfire")]
const HISTOGRAM_SCALE: i8 = 4;

/// Set once, after Logfire is configured. Instruments must never be built before that: the
/// SDK's global metric functions panic without a configured instance, and a token that is set
/// but fails to configure must not bring the TUI down.
#[cfg(feature = "logfire")]
static UI: std::sync::OnceLock<UiInstruments> = std::sync::OnceLock::new();

#[cfg(feature = "logfire")]
struct UiInstruments {
    turn: logfire::ExponentialHistogram<f64>,
    effects: logfire::ExponentialHistogram<f64>,
    draw: logfire::ExponentialHistogram<f64>,
    result_wait: logfire::ExponentialHistogram<f64>,
}

#[cfg(feature = "logfire")]
impl UiInstruments {
    fn new(l: &logfire::Logfire) -> Self {
        let m = l.metrics();
        let histogram = |name: &'static str, description: &'static str| {
            m.f64_exponential_histogram(name, HISTOGRAM_SCALE)
                .with_unit("ms")
                .with_description(description)
                .build()
        };
        UiInstruments {
            turn: histogram("ui.turn.duration", "Wake-up to frame written, by cause"),
            effects: histogram("ui.effects.duration", "Effects run on the UI loop"),
            draw: histogram("ui.draw.duration", "Render and terminal write"),
            result_wait: histogram(
                "ui.result.wait",
                "Task started to its result on screen, by task",
            ),
        }
    }

    fn record_turn(&self, turn: &Turn) {
        let cause = [opentelemetry::KeyValue::new("cause", turn.cause)];
        self.turn.record(ms(turn.total()), &cause);
        self.effects.record(ms(turn.effects), &[]);
        self.draw.record(ms(turn.draw), &[]);
    }
}

/// A token counts only if it is non-empty after trimming.
pub fn token_present(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

/// Whether telemetry would be exported in this process.
pub fn requested() -> bool {
    cfg!(feature = "logfire") && token_present(std::env::var("LOGFIRE_TOKEN").ok().as_deref())
}

/// Holds the exporter for the life of the process. Call [`Telemetry::shutdown`] before
/// exiting: `std::process::exit` skips destructors, so the `Drop` flush only covers unwinding.
pub struct Telemetry {
    #[cfg(feature = "logfire")]
    inner: Option<logfire::Logfire>,
}

/// How long exit may wait for pending spans. Exporting happens on a background thread; an
/// unreachable collector must not make the CLI hang.
#[cfg(feature = "logfire")]
const FLUSH_BUDGET: std::time::Duration = std::time::Duration::from_secs(3);

impl Telemetry {
    /// Telemetry that does nothing.
    pub fn off() -> Self {
        Telemetry {
            #[cfg(feature = "logfire")]
            inner: None,
        }
    }

    /// Whether spans are being exported.
    pub fn active(&self) -> bool {
        #[cfg(feature = "logfire")]
        {
            self.inner.is_some()
        }
        #[cfg(not(feature = "logfire"))]
        {
            false
        }
    }

    /// Starts exporting if `LOGFIRE_TOKEN` is set. A configuration problem is reported once on
    /// stderr and never stops the program. Must run inside a Tokio runtime: the SDK spawns its
    /// metric reader there, and panics without one.
    pub fn init() -> Self {
        #[cfg(feature = "logfire")]
        {
            if !requested() {
                return Telemetry::off();
            }
            match configure() {
                Ok(l) => {
                    let _ = UI.set(UiInstruments::new(&l));
                    return Telemetry { inner: Some(l) };
                }
                Err(e) => eprintln!("warning: logfire disabled: {e}"),
            }
        }
        Telemetry::off()
    }

    /// Flushes pending spans, waiting at most a few seconds.
    pub fn shutdown(mut self) {
        self.flush();
    }

    fn flush(&mut self) {
        #[cfg(feature = "logfire")]
        if let Some(l) = self.inner.take() {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(l.shutdown());
            });
            if rx.recv_timeout(FLUSH_BUDGET).is_err() {
                // collector unreachable or slow: give up rather than delay exit
            }
        }
    }
}

/// A panic unwinds past the explicit [`Telemetry::shutdown`] call; flushing here keeps the
/// spans that explain it. Normal exits have already flushed, so this does nothing for them.
impl Drop for Telemetry {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(feature = "logfire")]
fn configure() -> Result<logfire::Logfire, logfire::ConfigureError> {
    // `send_to_logfire` is left alone on purpose: setting it here would override the user's
    // LOGFIRE_SEND_TO_LOGFIRE (e.g. `no`). A token is already known to be present.
    let mut b = logfire::configure()
        .with_service_version(env!("CARGO_PKG_VERSION"))
        // The SDK defaults to TRACE when exporting, which would also ship hyper's and rustls's
        // connection-level events (hostnames, IP addresses, ports). Our spans are INFO; RUST_LOG
        // still overrides this for anyone who wants to debug.
        .with_default_level_filter(tracing::level_filters::LevelFilter::INFO)
        .with_console(None) // a full-screen TUI must never share the terminal with log output
        .with_metrics(Some(logfire::config::MetricsOptions::default())); // off by default
    if std::env::var_os("LOGFIRE_SERVICE_NAME").is_none() {
        b = b.with_service_name("rsbsky");
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_non_empty_token_turns_telemetry_on() {
        assert!(!token_present(None));
        assert!(!token_present(Some("")));
        assert!(!token_present(Some("  \t")));
        assert!(token_present(Some("pylf_v1_abc")));
    }

    #[test]
    fn a_turn_is_slow_from_the_threshold_on() {
        let turn = |ms| Turn {
            cause: "key",
            handle: Duration::from_millis(ms),
            effects: Duration::from_millis(ms),
            draw: Duration::from_millis(ms),
        };
        assert_eq!(turn(10).total(), Duration::from_millis(30));
        assert!(!turn(16).is_slow()); // 48 ms
        assert!(turn(17).is_slow()); // 51 ms
    }

    #[test]
    fn recording_without_logfire_configured_does_not_panic() {
        let turn = Turn {
            cause: "key",
            handle: Duration::from_secs(1), // slow, so the event path runs too
            ..Turn::default()
        };
        record_turn(&turn);
        record_result_wait("page", Duration::from_millis(5));
    }

    #[test]
    fn off_telemetry_is_inert() {
        let t = Telemetry::off();
        assert!(!t.active());
        t.shutdown();
    }
}
