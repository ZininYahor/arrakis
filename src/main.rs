// No console window on Windows; the app lives in the tray.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    arrakis::entrypoint::run()
}
