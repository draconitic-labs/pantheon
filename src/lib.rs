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

/// Say in the log why a test didn't really run. A passing test's output is hidden, so this
/// writes past the test harness's capture, and every skip shows however the tests are run.
#[cfg(test)]
pub(crate) fn test_skipped(why: &str) {
    use std::io::Write;
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("a test");
    // one write, so parallel tests can't interleave with it
    let line = format!("SKIPPED {test}: {why}\n");
    let _ = std::io::stderr().write_all(line.as_bytes());
}
