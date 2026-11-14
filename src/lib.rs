use std::path::PathBuf;

pub mod cli;

pub mod config;

pub mod platform;

pub mod steam;

pub struct ClientPaths {
    pub root: PathBuf,
    pub libraries: Vec<PathBuf>,
}

pub trait GameClient {
    fn detect() -> Option<Self>
    where
        Self: Sized;

    fn name(&self) -> &str;
    fn paths(&self) -> &ClientPaths;
}
