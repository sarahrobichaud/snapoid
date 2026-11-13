//! Entry point for the `yapzm` CLI.

/// Runs the command-line interface.
fn main() -> anyhow::Result<()> {
    yapzm::cli::run()
}
