//! keyrock-case-study — CLI entry point.
//!
//! Deliberately thin: parse arguments, initialise logging, delegate. Anything
//! worth testing belongs in the library crate (`src/lib.rs`), which the
//! integration tests in `tests/` can actually reach.

use anyhow::Result;
use clap::{Parser, Subcommand};
use keyrock_case_study::{config::Config, greeting, telemetry};

#[derive(Parser)]
#[command(name = "keyrock-case-study", version, about = "Keyrock case study")]
struct Cli {
    /// Log at DEBUG instead of the configured level.
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a greeting — the smoke test that the binary runs at all.
    Hello {
        /// Who to greet.
        #[arg(default_value = "world")]
        name: String,
    },
    /// Report resolved configuration and environment health.
    Doctor,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;

    telemetry::init(if cli.verbose {
        "debug"
    } else {
        &config.log_level
    });

    match cli.command {
        Command::Hello { name } => {
            // stdout: the answer. Logs go to stderr (see telemetry.rs).
            println!("{}", greeting(&name));
        }
        Command::Doctor => {
            println!("version:   {}", keyrock_case_study::VERSION);
            println!("log_level: {}", config.log_level);
            println!("host:      {}", config.host);
            println!("port:      {}", config.port);
        }
    }

    Ok(())
}
