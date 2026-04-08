use anyhow::Result;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::boundary::{CommandTracker, ShellIntegration};
use crate::config::Config;
use crate::session;

/// RAII guard — restores the terminal from raw mode on drop.
struct RawModeGuard;

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

pub fn start_session(name: Option<String>, shell: Option<String>) -> Result<()> {
    let cfg = Config::load().unwrap_or_default();
    let shell_cmd = shell
        .or(cfg.default_shell.clone())
        .unwrap_or_else(default_shell);
    let meta = session::create_session(name, &shell_cmd)?;
    let session_dir = session::session_dir(&meta.id)?;

    // Write shell integration init script (if available for this shell)
    let init_script_path = write_init_script(&session_dir, &shell_cmd);

    eprintln!("🟢 Session {} started (shell: {})", meta.id, shell_cmd);
    eprintln!("   Logging to: {}", session_dir.display());
    if init_script_path.is_some() {
        eprintln!("   Command tracking: enabled (OSC 133)");
    } else {
        eprintln!("   Command tracking: disabled (unsupported shell)");
    }

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));

    // Create pseudo-terminal
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;

    // Build shell command, injecting init script if available
    let mut cmd = build_shell_command(&shell_cmd, &init_script_path);
    cmd.cwd(std::env::current_dir().unwrap_or_default());
    cmd.env("TERMSNOOP_SESSION", &meta.id);

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    // Clone handles from master
    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    // Open log file
    let log_path = session_dir.join("output.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    // Enter raw mode
    crossterm::terminal::enable_raw_mode()?;
    let _raw_guard = RawModeGuard;

    let running = Arc::new(AtomicBool::new(true));

    // stdin → PTY
    let running_w = running.clone();
    let _stdin_thread = std::thread::spawn(move || {
        forward_stdin(writer, running_w);
    });

    // PTY → stdout + log + command boundary tracking
    let running_r = running.clone();
    let sd = session_dir.clone();
    let max_log = cfg.max_log_bytes;
    let reader_thread = std::thread::spawn(move || {
        forward_output(reader, log_file, running_r, &sd, max_log);
    });

    // Block until shell exits
    let _status = child.wait()?;
    running.store(false, Ordering::Relaxed);

    // Drop master so reader thread gets EOF
    drop(pair.master);
    let _ = reader_thread.join();

    // Restore terminal before printing (guard dropped here)
    drop(_raw_guard);

    // Clean up init script
    if let Some(p) = &init_script_path {
        let _ = std::fs::remove_file(p);
    }

    session::update_session_status(&meta.id, "exited")?;
    eprintln!("\n🔴 Session {} ended.", meta.id);

    Ok(())
}

// ---------------------------------------------------------------------------
// Shell integration helpers
// ---------------------------------------------------------------------------

/// Write the shell integration init script to the session directory.
fn write_init_script(session_dir: &PathBuf, shell: &str) -> Option<PathBuf> {
    let script = ShellIntegration::init_script(shell)?;

    let basename = std::path::Path::new(shell)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
        .to_lowercase();

    let ext = match basename.as_str() {
        "pwsh" | "powershell" => "ps1",
        "fish" => "fish",
        _ => "sh",
    };

    let path = session_dir.join(format!("init.{}", ext));
    std::fs::write(&path, &script).ok()?;
    Some(path)
}

/// Build a CommandBuilder that sources the init script on startup.
fn build_shell_command(shell: &str, init_script: &Option<PathBuf>) -> CommandBuilder {
    let init_path = match init_script {
        Some(p) => p,
        None => return CommandBuilder::new(shell),
    };

    let basename = std::path::Path::new(shell)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(shell)
        .to_lowercase();

    let init_str = init_path.to_string_lossy();

    match basename.as_str() {
        "pwsh" | "powershell" => {
            let mut cmd = CommandBuilder::new(shell);
            cmd.args(["-NoExit", "-Command", &format!(". '{}'", init_str)]);
            cmd
        }
        "bash" => {
            let mut cmd = CommandBuilder::new(shell);
            // Source user's bashrc then our init
            cmd.args(["--rcfile", &init_str]);
            cmd
        }
        "zsh" => {
            let mut cmd = CommandBuilder::new(shell);
            // Zsh: source init after normal startup via ZDOTDIR trick
            cmd.args(["-c", &format!("source '{}'; exec zsh -i", init_str)]);
            cmd
        }
        "fish" => {
            let mut cmd = CommandBuilder::new(shell);
            cmd.args(["--init-command", &format!("source '{}'", init_str)]);
            cmd
        }
        _ => CommandBuilder::new(shell),
    }
}

// ---------------------------------------------------------------------------
// I/O forwarding
// ---------------------------------------------------------------------------

fn forward_stdin(mut writer: Box<dyn Write + Send>, running: Arc<AtomicBool>) {
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut buf = [0u8; 1024];

    while running.load(Ordering::Relaxed) {
        match handle.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if writer.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

fn forward_output(
    mut reader: Box<dyn Read + Send>,
    mut log_file: std::fs::File,
    running: Arc<AtomicBool>,
    session_dir: &PathBuf,
    max_log_bytes: u64,
) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    let mut buf = [0u8; 4096];
    let mut written: u64 = 0;
    let mut tracker = CommandTracker::new(session_dir);

    while running.load(Ordering::Relaxed) {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                let _ = handle.write_all(chunk);
                let _ = handle.flush();
                if written < max_log_bytes {
                    let _ = log_file.write_all(chunk);
                    written += n as u64;
                }
                tracker.process(chunk);
            }
        }
    }
}

fn default_shell() -> String {
    if cfg!(windows) {
        "pwsh".into()
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
    }
}
