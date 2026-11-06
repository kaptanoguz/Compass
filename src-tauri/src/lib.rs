pub mod categories;
pub mod files;
pub mod jsonfile;
pub mod model;
pub mod net;
pub mod providers;
pub mod ranking;
pub mod ratings;
pub mod search;
pub mod settings;
pub mod titles;
pub mod torrent;
pub mod downloads;
pub mod library;
pub mod qbittorrent;
pub mod server;
#[cfg(test)]
mod e2e_tests;
pub mod desktop;

pub use desktop::run;
