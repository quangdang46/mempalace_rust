//! MemPalace CLI entry point
//!
//! Allows running as: cargo run -- --help

use anyhow::Result;
use mempalace_core::cli;

fn main() -> Result<()> {
    cli::run()
}
