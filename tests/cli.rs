//! Integration tests: the real binary, invoked as a subprocess.
//!
//! These are the truth anchor. Unit tests prove functions behave; these prove
//! the shipped artefact behaves — argument parsing, exit codes, and the
//! stdout/stderr split included.
//!
//! `env!("CARGO_BIN_EXE_<name>")` is resolved by Cargo at compile time and
//! points at the binary built for THIS test run. Hardcoding `target/debug/...`
//! silently tests a stale binary once anyone runs `--release`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_keyrock-case-study");

#[test]
fn hello_greets_and_exits_zero() {
    let out = Command::new(BIN)
        .args(["hello", "Keyrock"])
        .output()
        .expect("failed to run binary");

    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Keyrock"), "stdout was: {stdout}");
}

#[test]
fn doctor_reports_resolved_configuration() {
    let out = Command::new(BIN)
        .arg("doctor")
        .env("KEYROCK_PORT", "9999")
        .output()
        .expect("failed to run binary");

    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("9999"), "stdout was: {stdout}");
}

#[test]
fn invalid_port_fails_loudly_rather_than_defaulting() {
    let out = Command::new(BIN)
        .arg("doctor")
        .env("KEYROCK_PORT", "not-a-number")
        .output()
        .expect("failed to run binary");

    assert!(!out.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("KEYROCK_PORT"), "stderr was: {stderr}");
}

#[test]
fn logs_go_to_stderr_not_stdout() {
    // A CLI whose logs land in stdout cannot be piped into anything.
    let out = Command::new(BIN)
        .args(["--verbose", "hello"])
        .output()
        .expect("failed to run binary");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        stdout.lines().count(),
        1,
        "stdout carried more than the answer: {stdout}"
    );
}
