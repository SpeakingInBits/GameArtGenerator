// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Must run first: Velopack's installer and updater start the app with special arguments to run install/update
    // hooks, and it applies a downloaded update on startup.
    velopack::VelopackApp::build().run();
    game_art_generator_lib::run()
}
