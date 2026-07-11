//! Pantheon library crate — shared by the GUI binary (windows subsystem) and
//! the console CLI binary, and by the test harness.

pub mod app;
pub mod atlas;
#[cfg(windows)]
pub mod audio;
pub mod cli;
pub mod data;
pub mod engine;
pub mod entity;
pub mod frames;
pub mod sim;
pub mod theme;
