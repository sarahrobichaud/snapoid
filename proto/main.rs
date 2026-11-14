//! CLI layer: argument definitions and thin handlers that compose library
//! entry points.

mod cli;
mod report;
mod ui;

fn main() -> anyhow::Result<()> {
    cli::run()
}
