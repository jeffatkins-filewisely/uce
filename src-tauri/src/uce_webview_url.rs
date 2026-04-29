//! Shared URL checks for “main webview shows real app UI” (dev server vs packaged asset host).

pub fn url_looks_like_loaded_app_ui(url: &str) -> bool {
    let u = url.to_ascii_lowercase();
    // Packaged / IPC hosts
    if u.contains("tauri.localhost")
        || u.starts_with("http://tauri.localhost")
        || u.starts_with("https://tauri.localhost")
        || u.starts_with("tauri://localhost")
    {
        return true;
    }
    // Dev: loopback HTTP(S) with explicit host prefix (avoid substring false positives)
    if u.starts_with("http://127.0.0.1:")
        || u.starts_with("http://localhost:")
        || u.starts_with("https://127.0.0.1:")
        || u.starts_with("https://localhost:")
    {
        return true;
    }
    // Legacy explicit checks (subset of above)
    u.contains("127.0.0.1:5173") || u.contains("localhost:5173")
}
