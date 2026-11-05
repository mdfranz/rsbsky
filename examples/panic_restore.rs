//! Manual terminal-lifecycle check: enters the TUI terminal mode, then panics.
//! The terminal must come back usable (see scripts/tui_check.py).

fn main() {
    // like the real binary: telemetry (if LOGFIRE_TOKEN is set) installs its own panic hook first
    let _telemetry = rsbsky::telemetry::Telemetry::init();
    rsbsky::tui::runtime::install_panic_hook();
    let _guard = rsbsky::tui::runtime::TerminalGuard::enter().expect("terminal");
    println!("in alternate screen; panicking in 300ms");
    std::thread::sleep(std::time::Duration::from_millis(300));
    panic!("deliberate panic");
}
