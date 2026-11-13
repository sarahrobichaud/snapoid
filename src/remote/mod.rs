//! Remote Steam queries: everything yapzm learns from Steam at query
//! time, as opposed to the local scanners and store. All reads are
//! anonymous: they only touch public app metadata, so no account is
//! needed.
//!
//! - [`steamcmd`]: metadata reads driven through the steamcmd CLI
//!   (plumbing in [`crate::steamcmd`]).
//! - [`web`]: Steam Web API queries.

pub mod steamcmd;
pub mod web;

pub use steamcmd::public_build;
