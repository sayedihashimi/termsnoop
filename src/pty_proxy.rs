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

/// Forward input to the PTY. On Windows, uses ReadConsoleInputW to
/// properly handle arrow keys, function keys, and other special keys.
/// On Unix, reads raw bytes from stdin (already VT-encoded by the terminal).
fn forward_stdin(writer: Box<dyn Write + Send>, running: Arc<AtomicBool>, debug_log: DebugLog) {
    #[cfg(windows)]
    forward_stdin_win32(writer, running, debug_log);

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
mod win32 {
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct KeyEventRecord {
        pub key_down: i32,
        pub repeat_count: u16,
        pub virtual_key_code: u16,
        pub virtual_scan_code: u16,
        pub uchar: u16,
        pub control_key_state: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct InputRecord {
        pub event_type: u16,
        pub _padding: u16,
        pub event: [u8; 16],
    }

    pub const KEY_EVENT: u16 = 0x0001;
    pub const WAIT_TIMEOUT: u32 = 258;

    extern "system" {
        pub fn ReadConsoleInputW(
            handle: *mut std::ffi::c_void,
            buffer: *mut InputRecord,
            length: u32,
            events_read: *mut u32,
        ) -> i32;
        pub fn WaitForSingleObject(handle: *mut std::ffi::c_void, timeout_ms: u32) -> u32;
    }
}

/// Read console input events directly via Win32 API and translate to VT sequences.
/// This avoids crossterm's self-pipe mechanism which interferes with ConPTY.
#[cfg(windows)]
fn forward_stdin_win32(mut writer: Box<dyn Write + Send>, running: Arc<AtomicBool>, debug_log: DebugLog) {
    use std::os::windows::io::AsRawHandle;
    use win32::*;

    let stdin = std::io::stdin();
    let handle = stdin.as_raw_handle();

    if let Some(ref dl) = debug_log {
        debug_write(dl, &format!("stdin reader: using Win32 ReadConsoleInputW, handle={:?}", handle));
    }

    while running.load(Ordering::Relaxed) {
        // Wait with timeout so we can check `running`
        let wait_result = unsafe { WaitForSingleObject(handle, 50) };
        if wait_result == WAIT_TIMEOUT {
            continue;
        }

        let mut record = InputRecord {
            event_type: 0,
            _padding: 0,
            event: [0u8; 16],
        };
        let mut events_read: u32 = 0;

        let ok = unsafe {
            ReadConsoleInputW(handle, &mut record, 1, &mut events_read)
        };

        if ok == 0 || events_read == 0 {
            if let Some(ref dl) = debug_log {
                debug_write(dl, &format!("ReadConsoleInputW failed or no events (ok={}, read={})", ok, events_read));
            }
            break;
        }

        if record.event_type != KEY_EVENT {
            continue;
        }

        let key: KeyEventRecord = unsafe { std::ptr::read(record.event.as_ptr() as *const KeyEventRecord) };

        // Only process key-down events
        if key.key_down == 0 {
            continue;
        }

        if let Some(ref dl) = debug_log {
            debug_write(dl, &format!(
                "Key: vk=0x{:02X} scan=0x{:02X} char=0x{:04X}({}) mods=0x{:X} repeat={}",
                key.virtual_key_code,
                key.virtual_scan_code,
                key.uchar,
                if key.uchar >= 0x20 && key.uchar < 0x7F {
                    char::from(key.uchar as u8).to_string()
                } else {
                    "?".to_string()
                },
                key.control_key_state,
                key.repeat_count,
            ));
        }

        let bytes = win32_key_to_bytes(&key);

        if let Some(ref b) = bytes {
            if let Some(ref dl) = debug_log {
                let hex: Vec<String> = b.iter().map(|byte| format!("0x{:02X}", byte)).collect();
                debug_write(dl, &format!("  -> sending: {}", hex.join(" ")));
            }
            for _ in 0..key.repeat_count.max(1) {
                if writer.write_all(b).is_err() {
                    return;
                }
            }
        }
    }
}

/// Translate a Win32 key event to VT escape sequence bytes.
#[cfg(windows)]
fn win32_key_to_bytes(key: &win32::KeyEventRecord) -> Option<Vec<u8>> {
    const VK_BACK: u16 = 0x08;
    const VK_TAB: u16 = 0x09;
    const VK_RETURN: u16 = 0x0D;
    const VK_ESCAPE: u16 = 0x1B;
    const VK_PRIOR: u16 = 0x21;  // Page Up
    const VK_NEXT: u16 = 0x22;   // Page Down
    const VK_END: u16 = 0x23;
    const VK_HOME: u16 = 0x24;
    const VK_LEFT: u16 = 0x25;
    const VK_UP: u16 = 0x26;
    const VK_RIGHT: u16 = 0x27;
    const VK_DOWN: u16 = 0x28;
    const VK_INSERT: u16 = 0x2D;
    const VK_DELETE: u16 = 0x2E;
    const VK_F1: u16 = 0x70;
    const VK_F12: u16 = 0x7B;

    const LEFT_CTRL: u32 = 0x0008;
    const RIGHT_CTRL: u32 = 0x0004;
    const LEFT_ALT: u32 = 0x0002;
    const RIGHT_ALT: u32 = 0x0001;
    const SHIFT: u32 = 0x0010;

    let ctrl = key.control_key_state & (LEFT_CTRL | RIGHT_CTRL) != 0;
    let alt = key.control_key_state & (LEFT_ALT | RIGHT_ALT) != 0;
    let shift = key.control_key_state & SHIFT != 0;

    // If the character is non-zero, it's a regular character input
    if key.uchar != 0 {
        let ch = key.uchar;
        match key.virtual_key_code {
            VK_RETURN => return Some(vec![b'\r']),
            VK_BACK => return Some(vec![0x7f]),
            VK_TAB => {
                if shift {
                    return Some(b"\x1b[Z".to_vec());
                }
                return Some(vec![b'\t']);
            }
            VK_ESCAPE => return Some(vec![0x1b]),
            _ => {}
        }

        // Ctrl+key combinations (char value is already the control code)
        if ctrl && ch < 0x20 {
            if alt {
                return Some(vec![0x1b, ch as u8]);
            }
            return Some(vec![ch as u8]);
        }

        // Alt+key
        if alt && !ctrl {
            if let Some(c) = char::from_u32(ch as u32) {
                let mut buf = vec![0x1b];
                let mut cbuf = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut cbuf).as_bytes());
                return Some(buf);
            }
        }

        // Regular character (may be Unicode)
        if let Some(c) = char::from_u32(ch as u32) {
            let mut buf = [0u8; 4];
            return Some(c.encode_utf8(&mut buf).as_bytes().to_vec());
        }
    }

    // Virtual key codes for special keys (no character generated)
    let modifier_param = if ctrl || alt || shift {
        let m = 1
            + if shift { 1 } else { 0 }
            + if alt { 2 } else { 0 }
            + if ctrl { 4 } else { 0 };
        Some(m)
    } else {
        None
    };

    match key.virtual_key_code {
        VK_UP => Some(arrow_with_mod(b'A', modifier_param)),
        VK_DOWN => Some(arrow_with_mod(b'B', modifier_param)),
        VK_RIGHT => Some(arrow_with_mod(b'C', modifier_param)),
        VK_LEFT => Some(arrow_with_mod(b'D', modifier_param)),
        VK_HOME => Some(b"\x1b[H".to_vec()),
        VK_END => Some(b"\x1b[F".to_vec()),
        VK_INSERT => Some(b"\x1b[2~".to_vec()),
        VK_DELETE => Some(b"\x1b[3~".to_vec()),
        VK_PRIOR => Some(b"\x1b[5~".to_vec()),
        VK_NEXT => Some(b"\x1b[6~".to_vec()),
        vk @ VK_F1..=VK_F12 => {
            let fkey = (vk - VK_F1 + 1) as u8;
            f_key_seq(fkey)
        }
        _ => None,
    }
}

#[cfg(windows)]
fn arrow_with_mod(dir: u8, modifier: Option<u32>) -> Vec<u8> {
    match modifier {
        None => vec![0x1b, b'[', dir],
        Some(m) => format!("\x1b[1;{}{}", m, dir as char).into_bytes(),
    }
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
