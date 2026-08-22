//! keyrock-case-study — library crate.
//!
//! Everything testable lives here. `src/main.rs` is a thin shell that parses
//! arguments and calls into this crate.
//!
//! WHY THE SPLIT: an integration test in `tests/` can only `use` a library
//! crate. Logic written directly in `main.rs` is reachable from unit tests in
//! that file and from nowhere else — so it silently stops being covered the
//! moment the test suite grows past one file.

pub mod config;
pub mod telemetry;

/// The crate version, taken from Cargo.toml at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns the greeting used by the `hello` command.
pub fn greeting(name: &str) -> String {
    format!("Hello, {name}! keyrock-case-study v{VERSION} is alive.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_includes_name_and_version() {
        let out = greeting("Keyrock");
        assert!(out.contains("Keyrock"));
        assert!(out.contains(VERSION));
    }
}
