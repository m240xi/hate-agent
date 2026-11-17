#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--mcp-web-search") {
        hate_app::run_web_search_mcp();
        return;
    }
    hate_app::run();
}
