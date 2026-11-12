//! Project Zomboid domain: game facts, version handling, and version
//! probing.

pub mod appmanifest;
pub mod compat;
pub mod item;
pub mod mod_info;
pub mod probe;
pub mod version;

/// Project Zomboid's Steam app ID.
pub const APP_ID: u32 = 108600;

/// The Windows depot holding the game files snapoid snapshots; the
/// default for `download_depot` when neither the CLI nor the registry
/// entry specifies one.
pub const WINDOWS_DEPOT: u64 = 108604;

/// Directory name of the game install under steamapps/common.
pub const COMMON_DIR: &str = "ProjectZomboid";

/// Top-level entries of the game install that never belong to a stored
/// version: standalone mods, and home-folder data the game may have
/// dropped into its working directory.
pub const EXCLUDED_INSTALL_ENTRIES: &[&str] = &["mods", "Zomboid"];
