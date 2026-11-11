//! Component 12: Terminal UI helpers.
//!
//! Interactive-console pieces shared by the CLI, built on the `dialoguer`
//! and `indicatif` crates: a menu picker for choosing from a list, and a
//! progress bar for long copy operations.

use std::io::IsTerminal;

use anyhow::Result;
use dialoguer::Select;
use indicatif::{ProgressBar, ProgressStyle};

/// Print `items` as an interactive menu and read a choice from the
/// terminal.
///
/// Returns the 0-based index. `default` is the pre-selected entry, so
/// pressing Enter picks it (callers pass the newest item). When stderr
/// is not an interactive terminal, an error is returned instead —
/// scripts should pass explicit values rather than answer menus.
pub fn pick(items: &[String], prompt: &str, default: usize) -> Result<usize> {
    if items.is_empty() {
        anyhow::bail!("nothing to pick from");
    }
    // dialoguer only checks the terminal, not stdin: with a real terminal
    // on stderr but piped stdin, it renders the menu and blocks forever.
    if !std::io::stdin().is_terminal() {
        anyhow::bail!("not an interactive terminal; pass an explicit value instead");
    }
    let selection = Select::new()
        .with_prompt(prompt)
        .items(items)
        .default(default)
        .interact_opt()?;
    match selection {
        Some(index) => Ok(index),
        None => anyhow::bail!("not an interactive terminal; pass an explicit value instead"),
    }
}

/// Progress bar for long copy operations, rendered on stderr as
/// `X / Y (p%)  N files`. indicatif hides the bar automatically when
/// stderr is not a terminal.
pub struct Progress {
    bar: ProgressBar,
    files: u64,
}

impl Progress {
    pub fn new(total_bytes: u64) -> Self {
        let bar = ProgressBar::new(total_bytes);
        bar.set_style(
            ProgressStyle::with_template("  {bytes} / {total_bytes} ({percent}%)  {msg}")
                .expect("progress template is valid"),
        );
        Self { bar, files: 0 }
    }

    /// A bar that renders nothing (for tests and internal moves).
    pub fn disabled() -> Self {
        Self {
            bar: ProgressBar::hidden(),
            files: 0,
        }
    }

    pub fn step(&mut self, bytes: u64) {
        self.files += 1;
        self.bar.inc(bytes);
        self.bar.set_message(format!("{} files", self.files));
    }

    /// Complete the bar and clear it from the terminal.
    pub fn finish(&mut self) {
        self.bar.finish_and_clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_progress_is_silent() {
        let mut progress = Progress::disabled();
        progress.step(100);
        progress.finish();
    }
}
