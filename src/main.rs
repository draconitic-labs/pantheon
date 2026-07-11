// GUI binary: windows subsystem in release so the desktop shortcut opens no
// console. CLI use is supported when output is piped, but `pantheon-cli` is
// the canonical console tool (its output is always visible).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pantheon::cli::entry();
}
