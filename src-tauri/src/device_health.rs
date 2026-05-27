//! Tray health (green / yellow / red) + heartbeat `device_health` payload for remote diagnostics.

use crate::ccc_package_sync;
use crate::connection_diagnostics;
use crate::tenant_config;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::image::Image;
use tauri::tray::TrayIconId;
use tauri::AppHandle;

const HEARTBEAT_STALE_MS: i64 = 12 * 60 * 1000; // 12 min (heartbeat interval is 5 min)

static PENDING_UPLOADS: AtomicU32 = AtomicU32::new(0);
static LAST_UPLOAD_UNIX_MS: AtomicI64 = AtomicI64::new(0);
static LAST_CCC_SYNC_UNIX_MS: AtomicI64 = AtomicI64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayHealthColor {
    Green,
    Yellow,
    Red,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceHealthSnapshot {
    pub tray_state: TrayHealthColor,
    pub tenant_configured: bool,
    pub business_id_short: String,
    pub heartbeat_ok: bool,
    pub heartbeat_stale: bool,
    pub last_heartbeat_unix_ms: i64,
    pub last_heartbeat_category: String,
    pub ccc_sync_paused: bool,
    pub ccc_sync_offline: bool,
    pub ccc_syncing_count: u32,
    pub last_ccc_sync_unix_ms: i64,
    pub pending_uploads: u32,
    pub last_upload_unix_ms: i64,
    pub agent_version: String,
}

fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub fn note_ccc_sync_activity() {
    LAST_CCC_SYNC_UNIX_MS.store(now_unix_ms(), Ordering::Relaxed);
}

fn business_id_short(bid: &str) -> String {
    let t = bid.trim();
    if t.len() >= 8 {
        format!("{}…", &t[..8])
    } else if t.is_empty() {
        "(not set)".to_string()
    } else {
        t.to_string()
    }
}

pub fn snapshot(app: &AppHandle) -> DeviceHealthSnapshot {
    let cfg = tenant_config::load_tenant_config(app).unwrap_or_default();
    let tenant_configured = !cfg.business_id.trim().is_empty()
        && !cfg.backend_url.trim().is_empty()
        && !cfg.anon_key.trim().is_empty();

    let hb = connection_diagnostics::heartbeat_outcome(app);
    let now = now_unix_ms();
    let hb_age = if hb.last_unix_ms > 0 {
        now.saturating_sub(hb.last_unix_ms)
    } else {
        i64::MAX
    };
    let heartbeat_ok = hb.success && hb.last_unix_ms > 0;
    let heartbeat_stale = hb.last_unix_ms > 0 && hb_age > HEARTBEAT_STALE_MS;

    let ccc_paused = ccc_package_sync::is_sync_paused();
    let ccc_offline = ccc_package_sync::is_ccc_offline();
    let ccc_syncing = ccc_package_sync::syncing_count();

    let tray_state = compute_tray_color(
        tenant_configured,
        ccc_paused,
        heartbeat_ok,
        hb.last_unix_ms,
        heartbeat_stale,
        hb.success,
        ccc_offline,
    );

    let version = app.package_info().version.to_string();

    DeviceHealthSnapshot {
        tray_state,
        tenant_configured,
        business_id_short: business_id_short(&cfg.business_id),
        heartbeat_ok,
        heartbeat_stale,
        last_heartbeat_unix_ms: hb.last_unix_ms,
        last_heartbeat_category: hb.category.clone(),
        ccc_sync_paused: ccc_paused,
        ccc_sync_offline: ccc_offline,
        ccc_syncing_count: ccc_syncing,
        last_ccc_sync_unix_ms: LAST_CCC_SYNC_UNIX_MS.load(Ordering::Relaxed),
        pending_uploads: PENDING_UPLOADS.load(Ordering::Relaxed),
        last_upload_unix_ms: LAST_UPLOAD_UNIX_MS.load(Ordering::Relaxed),
        agent_version: version,
    }
}

fn compute_tray_color(
    tenant_configured: bool,
    ccc_paused: bool,
    heartbeat_ok: bool,
    last_hb_ms: i64,
    heartbeat_stale: bool,
    last_hb_success: bool,
    ccc_offline: bool,
) -> TrayHealthColor {
    if !tenant_configured {
        return TrayHealthColor::Red;
    }
    if ccc_paused {
        return TrayHealthColor::Yellow;
    }
    if last_hb_ms == 0 {
        return TrayHealthColor::Yellow;
    }
    if heartbeat_stale || !last_hb_success {
        return TrayHealthColor::Red;
    }
    if !heartbeat_ok || ccc_offline {
        return TrayHealthColor::Yellow;
    }
    TrayHealthColor::Green
}

fn format_relative_ms(ms: i64) -> String {
    if ms <= 0 {
        return "never".to_string();
    }
    let age = now_unix_ms().saturating_sub(ms);
    if age < 60_000 {
        return "just now".to_string();
    }
    if age < 3_600_000 {
        return format!("{}m ago", age / 60_000);
    }
    if age < 86_400_000 {
        return format!("{}h ago", age / 3_600_000);
    }
    format!("{}d ago", age / 86_400_000)
}

pub fn tray_tooltip(snap: &DeviceHealthSnapshot) -> String {
    let status_word = match snap.tray_state {
        TrayHealthColor::Green => "Connected",
        TrayHealthColor::Yellow => {
            if snap.ccc_sync_paused {
                "Paused"
            } else {
                "Attention"
            }
        }
        TrayHealthColor::Red => "Disconnected",
    };

    let shop = if snap.tenant_configured {
        format!("Shop {}", snap.business_id_short)
    } else {
        "Not connected — tray → Connect".to_string()
    };

    let hb_line = if snap.last_heartbeat_unix_ms > 0 {
        format!(
            "Heartbeat: {} ({})",
            if snap.heartbeat_ok { "OK" } else { "failed" },
            format_relative_ms(snap.last_heartbeat_unix_ms)
        )
    } else {
        "Heartbeat: waiting for first success".to_string()
    };

    let ccc_line = if snap.ccc_sync_paused {
        "CCC sync: paused".to_string()
    } else if snap.ccc_sync_offline {
        "CCC sync: offline".to_string()
    } else if snap.ccc_syncing_count > 0 {
        format!("CCC sync: {} in progress", snap.ccc_syncing_count)
    } else {
        format!(
            "CCC sync: idle (last {})",
            format_relative_ms(snap.last_ccc_sync_unix_ms)
        )
    };

    let upload_line = if snap.pending_uploads > 0 {
        format!("Pending uploads: {}", snap.pending_uploads)
    } else {
        format!(
            "Pending uploads: 0 (last {})",
            format_relative_ms(snap.last_upload_unix_ms)
        )
    };

    format!(
        "FileWisely UCE — {status_word}\r\n{shop}\r\n{hb_line}\r\n{ccc_line}\r\n{upload_line}\r\nv{}",
        snap.agent_version
    )
}

fn circle_tray_icon(size: u32, r: u8, g: u8, b: u8) -> Image<'static> {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    let cx = size as f32 / 2.0;
    let cy = size as f32 / 2.0;
    let radius = size as f32 / 2.0 - 1.0;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - cx;
            let dy = y as f32 + 0.5 - cy;
            if dx * dx + dy * dy <= radius * radius {
                rgba.extend([r, g, b, 255]);
            } else {
                rgba.extend([0, 0, 0, 0]);
            }
        }
    }
    Image::new_owned(rgba, size, size)
}

fn icon_for_color(color: TrayHealthColor) -> Image<'static> {
    match color {
        TrayHealthColor::Green => circle_tray_icon(16, 34, 197, 94),
        TrayHealthColor::Yellow => circle_tray_icon(16, 234, 179, 8),
        TrayHealthColor::Red => circle_tray_icon(16, 239, 68, 68),
    }
}

/// Update tray tooltip, icon color, and CCC sync menu label.
pub fn refresh_tray(app: &AppHandle) {
    let snap = snapshot(app);
    let tip = tray_tooltip(&snap);
    let tid = TrayIconId::new("uce-main-tray");
    if let Some(tray) = app.tray_by_id(&tid) {
        let _ = tray.set_tooltip(Some(tip));
        let icon = icon_for_color(snap.tray_state);
        let _ = tray.set_icon(Some(icon));
    }
    ccc_package_sync::refresh_tray_ccc_sync_item(app);
    ccc_package_sync::refresh_pause_resume_menu();
}

pub fn spawn_tray_health_refresh_loop(app: AppHandle) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(30));
            let a = app.clone();
            let _ = app.run_on_main_thread(move || refresh_tray(&a));
        }
    });
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadQueueReport {
    pub pending_uploads: u32,
    #[serde(default)]
    pub last_upload_unix_ms: Option<i64>,
}

#[tauri::command]
pub fn uce_report_upload_queue_stats(report: UploadQueueReport) {
    PENDING_UPLOADS.store(report.pending_uploads, Ordering::Relaxed);
    if let Some(ms) = report.last_upload_unix_ms {
        if ms > 0 {
            LAST_UPLOAD_UNIX_MS.store(ms, Ordering::Relaxed);
        }
    }
}

#[tauri::command]
pub fn uce_get_device_health_snapshot(app: AppHandle) -> DeviceHealthSnapshot {
    snapshot(&app)
}

#[tauri::command]
pub fn uce_refresh_tray_health(app: AppHandle) -> Result<(), String> {
    refresh_tray(&app);
    Ok(())
}
