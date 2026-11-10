//! Component 11: Remote Steam queries.
//!
//! Everything snapoid learns from Steam at query time (as opposed to the
//! local scanners and store): each submodule owns one transport, assembles
//! its command lines, parses the answers, and exposes typed results, so
//! callers never touch raw output.
//!
//! - [`steamcmd`]: metadata reads driven through the steamcmd CLI
//!   (plumbing in [`crate::steamcmd`]).
//!
//! All reads are anonymous: they only touch public app metadata, so no
//! account is needed.

pub mod steamcmd;

pub use steamcmd::public_build;
