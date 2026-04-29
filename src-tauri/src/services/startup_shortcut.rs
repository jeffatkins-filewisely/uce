//! Per-user Startup shortcut so UCE relaunches after reboot (MSI installs skip `install.ps1`).

use std::path::PathBuf;
use std::process::Command;

/// Creates or updates `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Startup\FileWisely UCE.lnk`.
#[cfg(windows)]
pub fn ensure_filewisely_uce_shortcut() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let target = exe.to_string_lossy();
    let work_dir = exe
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());

    let appdata = std::env::var("APPDATA").map_err(|e| format!("APPDATA: {e}"))?;
    let lnk = PathBuf::from(appdata).join(
        "Microsoft\\Windows\\Start Menu\\Programs\\Startup\\FileWisely UCE.lnk",
    );
    let lnk_str = lnk.to_string_lossy().to_string();

    let ps = format!(
        "$w = New-Object -ComObject WScript.Shell; \
         $s = $w.CreateShortcut('{}'); \
         $s.TargetPath = '{}'; \
         $s.WorkingDirectory = '{}'; \
         $s.Description = 'FileWisely Universal Capture Engine'; \
         $s.Save()",
        escape_ps_single_quoted(&lnk_str),
        escape_ps_single_quoted(&target),
        escape_ps_single_quoted(&work_dir),
    );

    let mut ps_cmd = Command::new("powershell.exe");
    ps_cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-WindowStyle",
        "Hidden",
        "-Command",
        &ps,
    ]);
    let out = super::process_launch::run_output(
        "startup_shortcut",
        "create_startup_lnk",
        ps_cmd,
        super::process_launch::TIMEOUT_DEFAULT,
    )
    .map_err(|e| format!("powershell: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "Startup shortcut: powershell exited with {:?}",
            out.status.code()
        ));
    }

    // Backup autostart (MSI has no Inno `install.ps1`). If both Run + .lnk fire at logon, single-instance keeps one process.
    let reg_value = format!("\"{}\"", target.replace('"', ""));
    let mut reg_cmd = Command::new("reg.exe");
    reg_cmd.args([
        "add",
        r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
        "/v",
        "FileWiselyUCE",
        "/t",
        "REG_SZ",
        "/d",
        &reg_value,
        "/f",
    ]);
    match super::process_launch::run_output(
        "startup_shortcut",
        "hkcu_run_reg",
        reg_cmd,
        super::process_launch::TIMEOUT_DEFAULT,
    ) {
        Ok(o) if o.status.success() => {}
        Ok(o) => eprintln!(
            "[UCE] HKCU Run FileWiselyUCE: reg.exe failed (exit {:?}); Startup .lnk still created.",
            o.status.code()
        ),
        Err(e) => eprintln!(
            "[UCE] HKCU Run FileWiselyUCE: could not run reg.exe ({e}); Startup .lnk still created."
        ),
    }

    Ok(lnk_str)
}

/// PowerShell single-quoted string: `'` → `''`.
fn escape_ps_single_quoted(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(not(windows))]
pub fn ensure_filewisely_uce_shortcut() -> Result<String, String> {
    Err("Startup shortcut is only supported on Windows".to_string())
}
