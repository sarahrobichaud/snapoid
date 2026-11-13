//! The `yapzm` binary: entry point for the yapzm CLI. The executable's
//! name selects the app data dir (see `yapzm::config::app_dir_name`).

fn main() -> anyhow::Result<()> {
    yapzm::cli::run()
}
