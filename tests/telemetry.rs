//! Logfire telemetry against a fake OTLP collector (scripts/telemetry_check.py): inert without
//! a token, exports spans with one, never leaks credentials or identities, never changes a
//! command's output, and never hangs or draws on the TUI when the collector is unreachable.

use std::process::Command;

#[test]
fn logfire_export_is_opt_in_private_and_harmless() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found; skipping");
        return;
    }
    let root = env!("CARGO_MANIFEST_DIR");
    let mut cmd = Command::new("python3");
    cmd.arg(format!("{root}/scripts/telemetry_check.py"))
        .args(["--bin", env!("CARGO_BIN_EXE_rsbsky")]);
    if !cfg!(feature = "logfire") {
        cmd.arg("--disabled");
    }
    let out = cmd.output().expect("run telemetry_check.py");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "telemetry checks failed:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
