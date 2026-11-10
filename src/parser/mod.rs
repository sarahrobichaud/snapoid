//! Parsers for Steam and game file formats.
//!
//! - [`acf`] parses ACF/VDF files (appworkshop_<appid>.acf,
//!   libraryfolders.vdf, appmanifest_<appid>.acf).
//! - [`class`] parses JVM class files (jar contents).

pub mod acf;
pub mod class;
