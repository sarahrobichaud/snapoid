//! Components 2 and 4: everything that reads Steam's on-disk state.
//!
//! - [`steam`] locates the Steam install and derives PZ-relevant paths.
//! - [`workshop`] inventories installed workshop items and their mods.

pub mod steam;
pub mod workshop;
