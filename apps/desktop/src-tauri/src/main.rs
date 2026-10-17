// Release builds are GUI apps: no console window behind the main window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    stockwatcher_desktop_lib::run();
}
