//! External child processes: Windows **CREATE_NO_WINDOW**, bounded wait, stderr capture to logs.
//! Policy line: `UCE_PROCESS_LAUNCH_POLICY=hidden_no_console` (see [`log_startup_policy`]).

use std::io;
use std::process::{Command, Output, Stdio};
use std::sync::Once;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// `CREATE_NO_WINDOW` — do not allocate a console for `powershell.exe` / `cmd.exe` / etc.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub const TIMEOUT_DEFAULT: Duration = Duration::from_secs(15);
/// Elevated installer may wait on UAC.
pub const TIMEOUT_UAC_ASSIST: Duration = Duration::from_secs(120);
/// Large Office → PDF conversions.
pub const TIMEOUT_LIBREOFFICE: Duration = Duration::from_secs(300);

static POLICY_ONCE: Once = Once::new();

pub fn log_startup_policy() {
    POLICY_ONCE.call_once(|| {
        eprintln!(
            "UCE_PROCESS_LAUNCH_POLICY=hidden_no_console timeout_default_s=15 timeout_uac_s=120 timeout_libreoffice_s=300"
        );
    });
}

#[cfg(windows)]
pub fn apply_hidden(cmd: &mut Command) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub fn apply_hidden(_cmd: &mut Command) {}

fn sanitize_preview(s: &str, max: usize) -> String {
    let t = s.trim();
    let one_line: String = t.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if one_line.len() <= max {
        one_line
    } else {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    }
}

fn log_subprocess_result(
    module: &str,
    label: &str,
    program: &str,
    elapsed: Duration,
    exit: Option<i32>,
    timed_out: bool,
    stderr_snip: &str,
) {
    eprintln!(
        "UCE_SUBPROCESS module={} label={} program={} elapsed_ms={} exit={:?} timed_out={} stderr_preview={}",
        module,
        label,
        program,
        elapsed.as_millis(),
        exit,
        timed_out,
        sanitize_preview(stderr_snip, 400)
    );
}

#[cfg(windows)]
fn kill_process_tree(pid: u32) {
    let mut c = Command::new("taskkill.exe");
    c.args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    apply_hidden(&mut c);
    let _ = c.status();
}

#[cfg(not(windows))]
fn kill_process_tree(_pid: u32) {}

/// Run `cmd`, capture stdout/stderr, enforce **timeout**, never show a console on Windows.
pub fn run_output(
    module: &'static str,
    label: &str,
    mut cmd: Command,
    timeout: Duration,
) -> Result<Output, String> {
    apply_hidden(&mut cmd);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let program = cmd.get_program().to_string_lossy().into_owned();

    #[cfg(not(windows))]
    {
        let started = Instant::now();
        let out = cmd.output().map_err(|e| format!("{program}: {e}"))?;
        log_subprocess_result(
            module,
            label,
            &program,
            started.elapsed(),
            out.status.code(),
            false,
            &String::from_utf8_lossy(&out.stderr),
        );
        return Ok(out);
    }

    #[cfg(windows)]
    {
        let child = cmd
            .spawn()
            .map_err(|e| format!("spawn {program}: {e}"))?;
        let pid = child.id();

        let handle = thread::spawn(move || child.wait_with_output());

        let started = Instant::now();
        while !handle.is_finished() {
            if started.elapsed() > timeout {
                kill_process_tree(pid);
                let stderr_hint = match handle.join() {
                    Ok(Ok(ref o)) => String::from_utf8_lossy(&o.stderr).into_owned(),
                    _ => String::new(),
                };
                log_subprocess_result(
                    module,
                    label,
                    &program,
                    started.elapsed(),
                    None,
                    true,
                    &stderr_hint,
                );
                return Err(format!(
                    "UCE_SUBPROCESS_TIMEOUT module={module} label={label} program={program} pid={pid} after {timeout:?}"
                ));
            }
            thread::sleep(Duration::from_millis(40));
        }

        let out = handle
            .join()
            .map_err(|_| "subprocess thread panicked".to_string())
            .and_then(|r| r.map_err(|e: io::Error| e.to_string()))?;

        log_subprocess_result(
            module,
            label,
            &program,
            started.elapsed(),
            out.status.code(),
            false,
            &String::from_utf8_lossy(&out.stderr),
        );

        Ok(out)
    }
}
