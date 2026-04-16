// Prevents a console window from appearing on Windows
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    llm_relay_lib::run()
}
