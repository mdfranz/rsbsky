//! Manual terminal-lifecycle check: enters the TUI terminal mode, then panics.
//! The terminal must come back usable (see scripts/tui_check.py).

fn main() {
    rsbsky::tui::runtime::install_panic_hook();
    let _guard = rsbsky::tui::runtime::TerminalGuard::enter().expect("terminal");
    println!("in alternate screen; panicking in 300ms");
    std::thread::sleep(std::time::Duration::from_millis(300));
    panic!("deliberate panic");
}
