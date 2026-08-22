//! Integration tests: the real binary, invoked as a subprocess.
//!
//! These are the truth anchor. Unit tests prove functions behave; these prove
//! the shipped artefact behaves — argument parsing, exit codes, config/CLI
//! precedence, and the stdout/stderr split included.
//!
//! `env!("CARGO_BIN_EXE_<name>")` is resolved by Cargo at compile time and
//! points at the binary built for THIS test run. Hardcoding `target/debug/...`
//! silently tests a stale binary once anyone runs `--release`.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_keyrock-case-study");

#[test]
fn default_run_logs_defaults_with_empty_stdout() {
    let out = Command::new(BIN).output().expect("failed to run binary");

    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pair=ethbtc"), "stderr was: {stderr}");
    assert!(stderr.contains("port=50051"), "stderr was: {stderr}");
    assert!(
        out.stdout.is_empty(),
        "stdout was not empty: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn flags_override_defaults_with_no_env_vars() {
    let out = Command::new(BIN)
        .args(["--pair", "btcusd", "--port", "12345"])
        .output()
        .expect("failed to run binary");

    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pair=btcusd"), "stderr was: {stderr}");
    assert!(stderr.contains("port=12345"), "stderr was: {stderr}");
}

#[test]
fn env_vars_override_defaults_with_no_flags() {
    let out = Command::new(BIN)
        .env("KEYROCK_PAIR", "btcusd")
        .env("KEYROCK_PORT", "12345")
        .output()
        .expect("failed to run binary");

    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pair=btcusd"), "stderr was: {stderr}");
    assert!(stderr.contains("port=12345"), "stderr was: {stderr}");
}

#[test]
fn cli_flag_wins_over_env_var_for_port() {
    // The actual precedence regression test: KEYROCK_PORT and --port both
    // set, to different values — the flag must win.
    let out = Command::new(BIN)
        .env("KEYROCK_PORT", "1")
        .args(["--port", "12345"])
        .output()
        .expect("failed to run binary");

    assert!(out.status.success(), "exit status: {:?}", out.status);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("port=12345"), "stderr was: {stderr}");
    assert!(!stderr.contains("port=1 "), "stderr was: {stderr}");
}

#[test]
fn invalid_port_flag_is_rejected_by_clap() {
    let out = Command::new(BIN)
        .args(["--port", "not-a-number"])
        .output()
        .expect("failed to run binary");

    assert!(!out.status.success(), "expected a non-zero exit");
}

#[test]
fn invalid_port_env_var_fails_loudly_rather_than_defaulting() {
    let out = Command::new(BIN)
        .env("KEYROCK_PORT", "not-a-number")
        .output()
        .expect("failed to run binary");

    assert!(!out.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("KEYROCK_PORT"), "stderr was: {stderr}");
}
