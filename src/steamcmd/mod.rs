//! Compatibility shim: steamcmd lives under the Steam umbrella now
//! ([`crate::steam::steamcmd`]). This re-export keeps the old paths
//! working until call sites migrate.

pub use crate::steam::steamcmd::*;
