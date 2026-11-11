//! Terminal UI: interactive picker and the progress-bar adapter for
//! [`snapoid::fsops::Progress`]. Built on the `dialoguer` and `indicatif`
//! crates.

use std::io::IsTerminal;

use anyhow::Result;
use dialoguer::Select;
use indicatif::{ProgressBar, ProgressStyle};

use snapoid::fsops;

/// Print `items` as an interactive menu and read a choice from the
/// terminal.
///
/// Returns the 0-based index. `default` is the pre-selected entry, so
/// pressing Enter picks it (callers pass the newest item). When stdin
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
}

impl fsops::Progress for Progress {
    fn step(&mut self, bytes: u64) {
        self.files += 1;
        self.bar.inc(bytes);
        self.bar.set_message(format!("{} files", self.files));
    }

    fn finish(&mut self) {
        self.bar.finish_and_clear();
    }
}

/// Progress factory for store operations: a real terminal bar.
pub fn progress_factory(total_bytes: u64) -> Box<dyn fsops::Progress> {
    Box::new(Progress::new(total_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapoid::fsops::Progress as _;

    #[test]
    fn hidden_progress_is_silent() {
        let mut progress = Progress::new(0);
        progress.step(100);
        progress.finish();
    }
}
