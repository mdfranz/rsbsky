//! Runs the executable compatibility matrix (scripts/compat.py) against the built binary,
//! comparing it with the golden outputs captured from the Go bskyban baseline.

use std::process::Command;

#[test]
fn matches_go_baseline() {
    if Command::new("python3").arg("--version").output().is_err() {
        eprintln!("python3 not found; skipping compatibility matrix");
        return;
    }
    let root = env!("CARGO_MANIFEST_DIR");
    let out = Command::new("python3")
        .arg(format!("{root}/scripts/compat.py"))
        .args(["check", "--bin", env!("CARGO_BIN_EXE_rsbsky")])
        .output()
        .expect("run compat.py");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "compatibility matrix failed:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!("{text}");
}
