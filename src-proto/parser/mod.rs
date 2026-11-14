//! Parsers for Steam and game file formats.
//!
//! - [`acf`] parses ACF/VDF files (appworkshop_<appid>.acf,
//!   libraryfolders.vdf, appmanifest_<appid>.acf).
//! - [`lenient`] scans steamcmd's quoted-key tree output, which carries
//!   prompt noise the strict parser rejects.

pub mod acf;
pub mod lenient;
