//! Pantheon library crate — shared by the GUI binary (windows subsystem) and
//! the console CLI binary, and by the test harness.

pub mod app;
pub mod atlas;
#[cfg(feature = "audio")]
pub mod audio;
pub mod cli;
pub mod data;
pub mod engine;
pub mod entity;
pub mod evidence;
pub mod frames;
pub mod serve;
pub mod sim;
pub mod spice;
pub mod theme;
