//! Test build of the CLI: identical to `yapzm`, but the executable name
//! selects the `yapzm-test` store (see `yapzm::config::app_dir_name`).

fn main() -> anyhow::Result<()> {
    yapzm::cli::run()
}
