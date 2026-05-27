//! Localhost liveness probe for portal / installer QA: `GET http://127.0.0.1:49217/whoami`

use crate::device_id;
use serde::Serialize;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

pub const PROBE_ADDR: &str = "127.0.0.1:49217";

static PROBE_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
struct WhoamiResponse {
    ok: bool,
    product: &'static str,
    agent_version: String,
    device_id: Option<String>,
    pid: u32,
}

fn handle_client(mut stream: TcpStream, app: &AppHandle) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut buf = [0u8; 1024];
    let n = stream.read(&mut buf).unwrap_or(0);
    if n == 0 {
        return;
    }
    let req = String::from_utf8_lossy(&buf[..n]);
    if !req.starts_with("GET /whoami") && !req.starts_with("GET / ") && !req.starts_with("GET / HTTP") {
        let body = r#"{"ok":false,"error":"use GET /whoami"}"#;
        let resp = format!(
            "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = stream.write_all(resp.as_bytes());
        return;
    }

    let version = app.package_info().version.to_string();
    let device_id = device_id::load_device_id(app);
    let body = serde_json::to_string(&WhoamiResponse {
        ok: true,
        product: "FileWisely UCE",
        agent_version: version,
        device_id,
        pid: std::process::id(),
    })
    .unwrap_or_else(|_| r#"{"ok":false}"#.to_string());

    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
}

pub fn spawn_local_agent_probe(app: AppHandle) {
    if PROBE_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || {
        let listener = match TcpListener::bind(PROBE_ADDR) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("UCE_LOCAL_PROBE_BIND_FAILED {PROBE_ADDR} err={e}");
                PROBE_RUNNING.store(false, Ordering::SeqCst);
                return;
            }
        };
        eprintln!("UCE_LOCAL_PROBE_LISTENING http://{PROBE_ADDR}/whoami");
        for stream in listener.incoming().flatten() {
            let app_h = app.clone();
            thread::spawn(move || handle_client(stream, &app_h));
        }
    });
}
