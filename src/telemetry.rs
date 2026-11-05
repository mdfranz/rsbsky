//! Optional observability through [Pydantic Logfire](https://logfire.pydantic.dev).
//!
//! rsbsky is instrumented with the `tracing` crate (spans for commands, requests and TUI
//! background work), which costs nothing while no subscriber is installed. Exporting is
//! switched on **only** when `LOGFIRE_TOKEN` is set and non-empty, and only if the crate was
//! built with the `logfire` feature (the default).
//!
//! What is sent: the command, operation names, HTTP method/status, XRPC error *codes*,
//! counts, outcomes and timings. What is never sent: the app password, access tokens,
//! request URLs or bodies, handles, DIDs, post text, or error messages that could contain
//! them. The console exporter is disabled, so the terminal UI is never drawn over.
//!
//! Standard Logfire environment variables apply (`LOGFIRE_SERVICE_NAME`,
//! `LOGFIRE_ENVIRONMENT`, `LOGFIRE_BASE_URL`, `LOGFIRE_SEND_TO_LOGFIRE`, `RUST_LOG`).

/// A token counts only if it is non-empty after trimming.
pub fn token_present(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.trim().is_empty())
}

/// Whether telemetry would be exported in this process.
pub fn requested() -> bool {
    cfg!(feature = "logfire") && token_present(std::env::var("LOGFIRE_TOKEN").ok().as_deref())
}

/// Holds the exporter for the life of the process. Call [`Telemetry::shutdown`] before
/// exiting: `std::process::exit` skips destructors, so dropping is not enough to flush.
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
    /// stderr and never stops the program.
    pub fn init() -> Self {
        #[cfg(feature = "logfire")]
        {
            if !requested() {
                return Telemetry::off();
            }
            match configure() {
                Ok(l) => return Telemetry { inner: Some(l) },
                Err(e) => eprintln!("warning: logfire disabled: {e}"),
            }
        }
        Telemetry::off()
    }

    /// Flushes pending spans, waiting at most a few seconds.
    pub fn shutdown(self) {
        #[cfg(feature = "logfire")]
        if let Some(l) = self.inner {
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
        .with_console(None); // a full-screen TUI must never share the terminal with log output
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
    fn off_telemetry_is_inert() {
        let t = Telemetry::off();
        assert!(!t.active());
        t.shutdown();
    }
}
