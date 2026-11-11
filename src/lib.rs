//! Snapoid library: version management for Project Zomboid workshop
//! mods. The binary (`main.rs`) owns the CLI, rendering, and terminal
//! UI; this crate owns everything else.

pub mod config;
pub mod deploy;
pub mod download;
pub mod fmt;
pub mod fsops;
pub mod logger;
pub mod parser;
pub mod platform;
pub mod pz;
pub mod registry;
pub mod remote;
pub mod scanner;
pub mod steam;
pub mod steamcmd;
pub mod store;
