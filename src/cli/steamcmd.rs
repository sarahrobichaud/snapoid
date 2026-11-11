use snapoid::steamcmd::{self, SteamCmd as _};

/// Run steamcmd with the given arguments, or show the located binary when
/// there are none.
pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    let program = steamcmd::locate()?;
    if args.is_empty() {
        println!("steamcmd: {}", program.display());
        return Ok(());
    }
    let cmd = steamcmd::Process::new(program);
    cmd.run(&args)
}
