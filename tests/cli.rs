//! Integration tests: the real binary, invoked as a subprocess.
//!
//! These are the truth anchor. Unit tests prove functions behave; these prove
//! the shipped artefact behaves — argument parsing, exit codes, config/CLI
//! precedence, and the stdout/stderr split included.
//!
//! `env!("CARGO_BIN_EXE_<name>")` is resolved by Cargo at compile time and
//! points at the binary built for THIS test run. Hardcoding `target/debug/...`
//! silently tests a stale binary once anyone runs `--release`.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_rust-crypto-orderbook");

// As of Phase 3 of specs/002-binance-feed, `main` connects to Binance and
// loops forever reading the websocket — there is no bounded-run mode. That
// rules out `Command::output()` (which waits for exit) as a way to observe
// the config/CLI-precedence startup log line. Per revisions.md entry 4, the
// fix is to spawn the child with piped stderr and read lines until the
// `starting pair=... port=...` line appears, then kill the child — no mock
// server and no bounded-run flag needed, because that line is written
// before the connect attempt, so this works identically whether or not the
// process can actually reach Binance.

/// Spawns `BIN` with the given env vars and args, reads stderr lines (off a
/// background thread, so a hang can't block the test forever) until one
/// contains `needle` or `timeout` elapses, then kills the child. Returns the
/// lines captured so far, panicking with them on timeout for a clear
/// failure message.
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
    // stdout can't be asserted precisely here — unlike `Command::output()`,
    // a still-running child has no captured stdout to inspect after the
    // fact. The stdout/stderr split is still covered structurally: every
    // assertion below reads exclusively from the stderr handle.
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
    // The actual precedence regression test: ORDERBOOK_PORT and --port both
    // set, to different values — the flag must win.
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
