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

pub fn start_session(name: Option<String>, shell: Option<String>, debug: bool) -> Result<()> {
    let cfg = Config::load().unwrap_or_default();
    let shell_cmd = shell
        .or(cfg.default_shell.clone())
        .unwrap_or_else(default_shell);
    let meta = session::create_session(name, &shell_cmd)?;
    let session_dir = session::session_dir(&meta.id)?;

    // Write shell integration init script (if available for this shell)
    let init_script_path = write_init_script(&session_dir, &shell_cmd, cfg.command_history_size);

    // Open debug log if requested
    let debug_log = if debug {
        let home = dirs::home_dir().unwrap_or_default();
        let debug_path = home.join(".termsnoop").join("debug.log");
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&debug_path)?;
        eprintln!("   Debug log: {}", debug_path.display());
        Some(Arc::new(std::sync::Mutex::new(f)))
    } else {
        None
    };

    if let Some(ref dl) = debug_log {
        debug_write(dl, "=== Session started ===");
        debug_write(dl, &format!("Shell: {}", shell_cmd));
        debug_write(dl, &format!("OS: {}", std::env::consts::OS));
    }

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
    cmd.env(
        "TERMSNOOP_HISTORY_SIZE",
        &cfg.command_history_size.to_string(),
    );

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

    // Enter raw mode and enable VT input (so arrow keys send escape sequences)
    crossterm::terminal::enable_raw_mode()?;
    #[cfg(windows)]
    {
        enable_virtual_terminal_input();
        if let Some(ref dl) = debug_log {
            log_console_mode(dl);
        }
    }
    let _raw_guard = RawModeGuard;

    let running = Arc::new(AtomicBool::new(true));

    // stdin → PTY
    let running_w = running.clone();
    let dl_clone = debug_log.clone();
    let _stdin_thread = std::thread::spawn(move || {
        forward_stdin(writer, running_w, dl_clone);
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
fn write_init_script(session_dir: &PathBuf, shell: &str, history_size: usize) -> Option<PathBuf> {
    let script = ShellIntegration::init_script(shell, history_size)?;

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

type DebugLog = Option<Arc<std::sync::Mutex<std::fs::File>>>;

fn debug_write(log: &Arc<std::sync::Mutex<std::fs::File>>, msg: &str) {
    if let Ok(mut f) = log.lock() {
        let _ = writeln!(f, "[{}] {}", chrono::Utc::now().format("%H:%M:%S%.3f"), msg);
        let _ = f.flush();
    }
}

#[cfg(windows)]
fn log_console_mode(log: &Arc<std::sync::Mutex<std::fs::File>>) {
    use std::os::windows::io::AsRawHandle;
    extern "system" {
        fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
    }
    let stdin = std::io::stdin();
    let handle = stdin.as_raw_handle();
    let mut mode: u32 = 0;
    let result = unsafe { GetConsoleMode(handle, &mut mode) };
    debug_write(
        log,
        &format!(
            "Console mode: 0x{:04X} (GetConsoleMode returned {}), VT_INPUT={}",
            mode,
            result,
            if mode & 0x0200 != 0 { "ON" } else { "OFF" }
        ),
    );
}

// ---------------------------------------------------------------------------
// I/O forwarding
// ---------------------------------------------------------------------------

/// Forward input to the PTY. On Windows, uses crossterm's event reader to
/// properly handle arrow keys, function keys, and other special keys that
/// the raw Win32 console doesn't translate to VT sequences via ReadFile.
/// On Unix, reads raw bytes from stdin (already VT-encoded by the terminal).
fn forward_stdin(writer: Box<dyn Write + Send>, running: Arc<AtomicBool>, debug_log: DebugLog) {
    #[cfg(windows)]
    forward_stdin_events(writer, running, debug_log);

    #[cfg(not(windows))]
    forward_stdin_raw(writer, running, debug_log);
}

#[cfg(not(windows))]
fn forward_stdin_raw(mut writer: Box<dyn Write + Send>, running: Arc<AtomicBool>, debug_log: DebugLog) {
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let mut buf = [0u8; 1024];

    if let Some(ref dl) = debug_log {
        debug_write(dl, "stdin reader: using raw mode (Unix)");
    }

    while running.load(Ordering::Relaxed) {
        match handle.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if let Some(ref dl) = debug_log {
                    let hex: Vec<String> = buf[..n].iter().map(|b| format!("0x{:02X}", b)).collect();
                    debug_write(dl, &format!("stdin raw: {} bytes: {}", n, hex.join(" ")));
                }
                if writer.write_all(&buf[..n]).is_err() {
                    break;
                }
            }
        }
    }
}

#[cfg(windows)]
fn forward_stdin_events(mut writer: Box<dyn Write + Send>, running: Arc<AtomicBool>, debug_log: DebugLog) {
    use crossterm::event::{self, Event, KeyEvent};

    if let Some(ref dl) = debug_log {
        debug_write(dl, "stdin reader: using crossterm event reader (Windows)");
    }

    while running.load(Ordering::Relaxed) {
        // Poll with timeout so we can check the `running` flag
        match event::poll(std::time::Duration::from_millis(50)) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => break,
        }

        let event = match event::read() {
            Ok(e) => e,
            Err(e) => {
                if let Some(ref dl) = debug_log {
                    debug_write(dl, &format!("event::read() error: {}", e));
                }
                break;
            }
        };

        if let Some(ref dl) = debug_log {
            debug_write(dl, &format!("Event: {:?}", event));
        }

        let bytes: Option<Vec<u8>> = match event {
            Event::Key(KeyEvent {
                code, modifiers, ..
            }) => key_to_vt(code, modifiers),
            Event::Paste(text) => Some(text.into_bytes()),
            _ => None, // ignore resize, focus, mouse
        };

        if let Some(ref b) = bytes {
            if let Some(ref dl) = debug_log {
                let hex: Vec<String> = b.iter().map(|byte| format!("0x{:02X}", byte)).collect();
                debug_write(dl, &format!("  -> sending {} bytes: {}", b.len(), hex.join(" ")));
            }
            if writer.write_all(b).is_err() {
                if let Some(ref dl) = debug_log {
                    debug_write(dl, "  -> write to PTY failed!");
                }
                break;
            }
        } else if let Some(ref dl) = debug_log {
            debug_write(dl, "  -> (no bytes to send)");
        }
    }
}

/// Translate a crossterm key event into the VT escape sequence bytes
/// that a Unix terminal would send.
#[cfg(windows)]
fn key_to_vt(code: crossterm::event::KeyCode, mods: crossterm::event::KeyModifiers) -> Option<Vec<u8>> {
    use crossterm::event::{KeyCode::*, KeyModifiers};

    match code {
        Char(c) => {
            if mods.contains(KeyModifiers::CONTROL) {
                // Ctrl+A = 0x01 .. Ctrl+Z = 0x1A
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    Some(vec![lower as u8 - b'a' + 1])
                } else {
                    // Ctrl with non-alpha (e.g. Ctrl+[, Ctrl+])
                    let mut buf = [0u8; 4];
                    Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
                }
            } else if mods.contains(KeyModifiers::ALT) {
                // Alt+key = ESC followed by the character
                let mut buf = vec![0x1b];
                let mut cbuf = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut cbuf).as_bytes());
                Some(buf)
            } else {
                let mut buf = [0u8; 4];
                Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
            }
        }
        Enter => Some(vec![b'\r']),
        Backspace => Some(vec![0x7f]),
        Tab => {
            if mods.contains(KeyModifiers::SHIFT) {
                Some(b"\x1b[Z".to_vec()) // reverse tab
            } else {
                Some(vec![b'\t'])
            }
        }
        Esc => Some(vec![0x1b]),
        Up => Some(arrow_seq(b'A', mods)),
        Down => Some(arrow_seq(b'B', mods)),
        Right => Some(arrow_seq(b'C', mods)),
        Left => Some(arrow_seq(b'D', mods)),
        Home => Some(b"\x1b[H".to_vec()),
        End => Some(b"\x1b[F".to_vec()),
        PageUp => Some(b"\x1b[5~".to_vec()),
        PageDown => Some(b"\x1b[6~".to_vec()),
        Insert => Some(b"\x1b[2~".to_vec()),
        Delete => Some(b"\x1b[3~".to_vec()),
        F(n) => f_key_seq(n),
        _ => None,
    }
}

/// Arrow key with optional Shift/Ctrl/Alt modifiers.
#[cfg(windows)]
fn arrow_seq(dir: u8, mods: crossterm::event::KeyModifiers) -> Vec<u8> {
    use crossterm::event::KeyModifiers;
    if mods.is_empty() {
        return vec![0x1b, b'[', dir];
    }
    // Modified arrows: ESC [ 1 ; <mod> <dir>
    let m = 1
        + if mods.contains(KeyModifiers::SHIFT) { 1 } else { 0 }
        + if mods.contains(KeyModifiers::ALT) { 2 } else { 0 }
        + if mods.contains(KeyModifiers::CONTROL) { 4 } else { 0 };
    format!("\x1b[1;{}{}", m, dir as char).into_bytes()
}

#[cfg(windows)]
fn f_key_seq(n: u8) -> Option<Vec<u8>> {
    let s = match n {
        1 => "\x1bOP",
        2 => "\x1bOQ",
        3 => "\x1bOR",
        4 => "\x1bOS",
        5 => "\x1b[15~",
        6 => "\x1b[17~",
        7 => "\x1b[18~",
        8 => "\x1b[19~",
        9 => "\x1b[20~",
        10 => "\x1b[21~",
        11 => "\x1b[23~",
        12 => "\x1b[24~",
        _ => return None,
    };
    Some(s.as_bytes().to_vec())
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

/// Enable ENABLE_VIRTUAL_TERMINAL_INPUT on Windows so arrow keys, Home, End,
/// etc. are sent as VT escape sequences through the PTY.
#[cfg(windows)]
fn enable_virtual_terminal_input() {
    use std::os::windows::io::AsRawHandle;

    extern "system" {
        fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
    }

    let stdin = std::io::stdin();
    let handle = stdin.as_raw_handle();

    unsafe {
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) != 0 {
            const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;
            let _ = SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_INPUT);
        }
    }
}
