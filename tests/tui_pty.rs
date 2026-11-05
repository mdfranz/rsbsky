//! Real-terminal checks for the feed TUI (scripts/tui_check.py): hyperlink bytes, no
//! terminal queries, clean shutdown (fast, default), and the full tmux suite covering
//! terminal restoration after quit / Ctrl+C / SIGTERM / panic / slow HTTP, resize, NO_COLOR,
//! background refresh and audit round-trips (slow: `cargo test -- --ignored`).

use std::process::Command;

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

fn run(args: &[&str]) {
    let root = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("python3")
        .arg(format!("{root}/scripts/tui_check.py"))
        .args(["--bin", env!("CARGO_BIN_EXE_rsbsky")])
        .args(args)
        .output()
        .expect("run tui_check.py");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "terminal checks failed:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn raw_terminal_bytes() {
    if !have("python3") {
        eprintln!("python3 not found; skipping");
        return;
    }
    run(&["--only-raw"]);
}

#[test]
#[ignore = "slow (~90s) and needs tmux; run with `cargo test -- --ignored`"]
fn tmux_terminal_lifecycle_suite() {
    assert!(have("tmux") && have("python3"), "needs tmux and python3");
    let root = env!("CARGO_MANIFEST_DIR");
    let panic_bin = format!("{root}/target/debug/examples/panic_restore");
    let built = Command::new(env!("CARGO"))
        .args(["build", "--examples", "-q"])
        .status()
        .unwrap();
    assert!(built.success());
    run(&["--panic-bin", &panic_bin]);
}
