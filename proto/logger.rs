//! Minimal leveled logger writing to stderr.
//!
//! Set the maximum level once at startup with [`init`], then use the
//! [`error!`], [`warn!`], [`info!`], and [`debug!`] macros anywhere in the
//! crate. Messages below the configured level are discarded.

use std::io::Write;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }
}

/// Current maximum level; 4 means "nothing enabled yet" (default: info).
static MAX_LEVEL: AtomicU8 = AtomicU8::new(2);

/// Enable logging up to and including `max_level`.
pub fn init(max_level: Level) {
    MAX_LEVEL.store(max_level as u8, Ordering::Relaxed);
}

fn enabled(level: Level) -> bool {
    level as u8 <= MAX_LEVEL.load(Ordering::Relaxed)
}

pub fn log(level: Level, args: std::fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "[{}] {}", level.as_str(), args);
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Error, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Warn, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Info, format_args!($($arg)*)) };
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => { $crate::logger::log($crate::logger::Level::Debug, format_args!($($arg)*)) };
}
