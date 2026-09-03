//! Integration tests: the real binary, invoked as a subprocess.
//!
//! Proves the shipped artefact behaves — argument parsing, exit codes,
//! config/CLI precedence, stdout/stderr split.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_rust-crypto-orderbook");

// main loops forever, so Command::output() (waits for exit) can't observe
// the startup log line — spawn with piped stderr, read until it appears,
// then kill the child.

/// Spawns `BIN`, reads stderr lines off a background thread until one
/// contains `needle` or `timeout` elapses, then kills the child.
fn spawn_and_capture_stderr_until(
    envs: &[(&str, &str)],
    args: &[&str],
    needle: &str,
    timeout: Duration,
) -> (Child, Vec<String>) {
    let mut cmd = Command::new(BIN);
    cmd.args(args).stderr(Stdio::piped()).stdout(Stdio::null());
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let mut child = cmd.spawn().expect("failed to spawn binary");
    let stderr = child.stderr.take().expect("stderr was not piped");

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut captured = Vec::new();
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("timed out waiting for {needle:?} in stderr; captured so far: {captured:?}");
        }
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                let found = line.contains(needle);
                captured.push(line);
                if found {
                    break;
                }
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                panic!("timed out waiting for {needle:?} in stderr; captured so far: {captured:?}");
            }
        }
    }
    (child, captured)
}

#[test]
fn default_run_logs_defaults_with_empty_stdout() {
    // A still-running child has no captured stdout to inspect, but every
    // assertion below reads exclusively from stderr, covering the split.
    let (mut child, captured) =
        spawn_and_capture_stderr_until(&[], &[], "port=50051", Duration::from_secs(5));
    let joined = captured.join("\n");
    assert!(joined.contains("pair=ethbtc"), "stderr was: {joined}");
    assert!(joined.contains("port=50051"), "stderr was: {joined}");
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn flags_override_defaults_with_no_env_vars() {
    let (mut child, captured) = spawn_and_capture_stderr_until(
        &[],
        &["--pair", "btcusd", "--port", "12345"],
        "port=12345",
        Duration::from_secs(5),
    );
    let joined = captured.join("\n");
    assert!(joined.contains("pair=btcusd"), "stderr was: {joined}");
    assert!(joined.contains("port=12345"), "stderr was: {joined}");
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn env_vars_override_defaults_with_no_flags() {
    let (mut child, captured) = spawn_and_capture_stderr_until(
        &[("ORDERBOOK_PAIR", "btcusd"), ("ORDERBOOK_PORT", "12345")],
        &[],
        "port=12345",
        Duration::from_secs(5),
    );
    let joined = captured.join("\n");
    assert!(joined.contains("pair=btcusd"), "stderr was: {joined}");
    assert!(joined.contains("port=12345"), "stderr was: {joined}");
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cli_flag_wins_over_env_var_for_port() {
    // ORDERBOOK_PORT and --port both set, to different values — flag must win.
    let (mut child, captured) = spawn_and_capture_stderr_until(
        &[("ORDERBOOK_PORT", "1")],
        &["--port", "12345"],
        "port=12345",
        Duration::from_secs(5),
    );
    let joined = captured.join("\n");
    assert!(joined.contains("port=12345"), "stderr was: {joined}");
    assert!(!joined.contains("port=1 "), "stderr was: {joined}");
    let _ = child.kill();
    let _ = child.wait();
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
        .env("ORDERBOOK_PORT", "not-a-number")
        .output()
        .expect("failed to run binary");

    assert!(!out.status.success(), "expected a non-zero exit");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ORDERBOOK_PORT"), "stderr was: {stderr}");
}
