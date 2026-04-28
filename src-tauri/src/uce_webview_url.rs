//! Shared URL checks for “main webview shows real app UI” (dev server vs packaged asset host).

pub fn url_looks_like_loaded_app_ui(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    u.contains("127.0.0.1:5173")
        || u.contains("localhost:5173")
        || u.contains("tauri.localhost")
        || u.starts_with("http://tauri.localhost")
        || u.starts_with("https://tauri.localhost")
        || u.starts_with("tauri://localhost")
}
