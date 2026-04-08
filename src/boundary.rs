use chrono::Utc;
use serde_json;
use std::io::Write;
use std::path::PathBuf;

use crate::session::CommandEntry;

// ---------------------------------------------------------------------------
// OSC 133 sequences (FinalTerm / shell integration)
//
//   ESC ] 133 ; A BEL   — prompt start
//   ESC ] 133 ; B BEL   — prompt end (user input begins)
//   ESC ] 133 ; C BEL   — command output starts (user pressed Enter)
//   ESC ] 133 ; D ; N BEL — command finished, exit code N
//
// BEL = 0x07, but ST (ESC \) is also accepted as terminator.
// ---------------------------------------------------------------------------

/// Shell integration init scripts per shell.
pub struct ShellIntegration;

impl ShellIntegration {
    /// Returns an init script for the given shell that injects OSC 133 markers.
    /// Returns `None` if the shell is unsupported.
    pub fn init_script(shell: &str) -> Option<String> {
        let basename = std::path::Path::new(shell)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(shell)
            .to_lowercase();

        match basename.as_str() {
            "pwsh" | "powershell" => Some(Self::pwsh_init()),
            "bash" => Some(Self::bash_init()),
            "zsh" => Some(Self::zsh_init()),
            "fish" => Some(Self::fish_init()),
            _ => None,
        }
    }

    fn pwsh_init() -> String {
        r#"
# termsnoop shell integration (OSC 133)
# Uses history tracking to only emit D markers after actual command execution,
# avoiding false triggers from prompt redraws (starship, oh-my-posh, etc.)

$__termsnoop_orig_prompt = $function:prompt
$global:__termsnoop_last_hist_id = -1

function prompt {
    $__tsExit = if ($global:?) { 0 } else { if ($global:LASTEXITCODE) { $global:LASTEXITCODE } else { 1 } }
    $curHist = (Get-History -Count 1 -ErrorAction SilentlyContinue)
    $curId = if ($curHist) { $curHist.Id } else { 0 }

    # Only emit D when a new command was actually executed (history ID advanced)
    if ($curId -gt $global:__termsnoop_last_hist_id -and $global:__termsnoop_last_hist_id -ge 0) {
        [Console]::Write("`e]133;D;$__tsExit`a")
    }
    $global:__termsnoop_last_hist_id = $curId

    # A: prompt start
    [Console]::Write("`e]133;A`a")
    $result = & $__termsnoop_orig_prompt
    # B embedded in return value so it appears after all prompt text
    if ($result) { return "$result`e]133;B`a" } else { return "`e]133;B`a" }
}
"#
        .to_string()
    }

    fn bash_init() -> String {
        r#"
# termsnoop shell integration (OSC 133)
__termsnoop_preexec() {
    printf '\e]133;C\a'
}
trap '__termsnoop_preexec' DEBUG

__termsnoop_orig_ps1="$PS1"
PS1='\[\e]133;D;$?\a\]\[\e]133;A\a\]'"$__termsnoop_orig_ps1"'\[\e]133;B\a\]'
"#
        .to_string()
    }

    fn zsh_init() -> String {
        r#"
# termsnoop shell integration (OSC 133)
__termsnoop_precmd() {
    print -Pn "\e]133;D;$?\a"
    print -Pn "\e]133;A\a"
}

__termsnoop_preexec() {
    print -Pn "\e]133;C\a"
}

precmd_functions+=(__termsnoop_precmd)
preexec_functions+=(__termsnoop_preexec)

# Append B marker after prompt
PS1="$PS1%{\e]133;B\a%}"
"#
        .to_string()
    }

    fn fish_init() -> String {
        r#"
# termsnoop shell integration (OSC 133)
function __termsnoop_prompt --on-event fish_prompt
    set -l last $status
    printf "\e]133;D;%s\a" $last
    printf "\e]133;A\a"
end

function __termsnoop_postprompt --on-event fish_postexec
    # intentionally empty — just needed for event registration
end

# Wrap the existing fish_prompt to append B marker
functions -c fish_prompt __termsnoop_orig_prompt 2>/dev/null
function fish_prompt
    __termsnoop_orig_prompt
    printf "\e]133;B\a"
end

function __termsnoop_preexec --on-event fish_preexec
    printf "\e]133;C\a"
end
"#
        .to_string()
    }
}

// ---------------------------------------------------------------------------
// OSC 133 streaming parser
// ---------------------------------------------------------------------------

/// Events emitted by the boundary detector.
#[derive(Debug)]
pub enum BoundaryEvent {
    PromptStart,                     // A
    PromptEnd,                       // B — user can type
    CommandStart,                    // C — user pressed Enter
    CommandDone { exit_code: i32 },  // D;N
}

/// Parser states for extracting OSC 133 sequences from a byte stream.
#[derive(Debug, Clone, PartialEq)]
enum State {
    Normal,
    Esc,            // saw 0x1B
    OscStart,       // saw 0x1B ]
    Osc133Semi,     // saw 0x1B ] 1 3 3 ;
    OscPayload,     // collecting marker char + optional params until BEL/ST
}

/// Streaming parser for OSC 133 sequences.
pub struct BoundaryParser {
    state: State,
    buf: Vec<u8>,          // accumulates the "133;" portion and payload
    osc_accum: Vec<u8>,    // accumulates chars after ESC ] to check for "133;"
}

impl BoundaryParser {
    pub fn new() -> Self {
        Self {
            state: State::Normal,
            buf: Vec::with_capacity(64),
            osc_accum: Vec::with_capacity(8),
        }
    }

    /// Feed a chunk of bytes and return any boundary events found.
    pub fn feed(&mut self, data: &[u8]) -> Vec<BoundaryEvent> {
        let mut events = Vec::new();

        for &b in data {
            match self.state {
                State::Normal => {
                    if b == 0x1B {
                        self.state = State::Esc;
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.state = State::OscStart;
                        self.osc_accum.clear();
                    } else {
                        self.state = State::Normal;
                    }
                }
                State::OscStart => {
                    // Accumulate until we see "133;" or know it's not ours
                    self.osc_accum.push(b);
                    let acc = &self.osc_accum;
                    let prefix = b"133;";
                    if acc.len() <= prefix.len() {
                        if acc[..] == prefix[..acc.len()] {
                            if acc.len() == prefix.len() {
                                // Matched "133;" — now collect payload
                                self.state = State::Osc133Semi;
                                self.buf.clear();
                            }
                        } else {
                            // Mismatch
                            self.state = State::Normal;
                            self.osc_accum.clear();
                        }
                    } else {
                        self.state = State::Normal;
                        self.osc_accum.clear();
                    }
                }
                State::Osc133Semi => {
                    if b == 0x07 {
                        // BEL — end of OSC
                        if let Some(ev) = self.parse_payload() {
                            events.push(ev);
                        }
                        self.state = State::Normal;
                    } else if b == 0x1B {
                        // Could be start of ST (\x1B\\)
                        self.state = State::OscPayload;
                    } else {
                        self.buf.push(b);
                    }
                }
                State::OscPayload => {
                    // Expecting '\\' to complete ST
                    if b == b'\\' {
                        if let Some(ev) = self.parse_payload() {
                            events.push(ev);
                        }
                    }
                    self.state = State::Normal;
                }
            }
        }

        events
    }

    /// Whether the parser is currently inside an escape/OSC sequence.
    #[allow(dead_code)]
    pub fn in_sequence(&self) -> bool {
        self.state != State::Normal
    }

    fn parse_payload(&self) -> Option<BoundaryEvent> {
        if self.buf.is_empty() {
            return None;
        }
        match self.buf[0] {
            b'A' => Some(BoundaryEvent::PromptStart),
            b'B' => Some(BoundaryEvent::PromptEnd),
            b'C' => Some(BoundaryEvent::CommandStart),
            b'D' => {
                let exit_code = if self.buf.len() > 2 && self.buf[1] == b';' {
                    std::str::from_utf8(&self.buf[2..])
                        .ok()
                        .and_then(|s| s.trim().parse::<i32>().ok())
                        .unwrap_or(0)
                } else {
                    0
                };
                Some(BoundaryEvent::CommandDone { exit_code })
            }
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Command tracker — uses parser events to build CommandEntry records
// ---------------------------------------------------------------------------

/// Tracks command boundaries and writes structured entries to commands.jsonl.
pub struct CommandTracker {
    parser: BoundaryParser,
    state: TrackerState,
    command_index: usize,
    current_input: Vec<u8>,   // raw bytes between B and C (user's typed command)
    current_output: Vec<u8>,  // raw bytes between C and next D/A
    commands_path: PathBuf,
    active: bool,             // true once we've seen at least one A marker
}

#[derive(Debug, PartialEq)]
enum TrackerState {
    Idle,       // waiting for first prompt
    InPrompt,   // between A and B
    InInput,    // between B and C (user typing)
    InOutput,   // between C and D (command output)
}

impl CommandTracker {
    pub fn new(session_dir: &PathBuf) -> Self {
        Self {
            parser: BoundaryParser::new(),
            state: TrackerState::Idle,
            command_index: 0,
            current_input: Vec::new(),
            current_output: Vec::new(),
            commands_path: session_dir.join("commands.jsonl"),
            active: false,
        }
    }

    /// Process a chunk of PTY output bytes. Call this for every read from the PTY.
    /// All bytes are accumulated to the current state buffer; ANSI sequences
    /// (including OSC 133 markers) are stripped later in flush_command().
    pub fn process(&mut self, data: &[u8]) {
        for &b in data {
            let events = self.parser.feed(&[b]);

            // Accumulate ALL bytes (including escape sequences) based on state.
            // strip_ansi_escapes handles cleanup in flush_command().
            match self.state {
                TrackerState::InInput => self.current_input.push(b),
                TrackerState::InOutput => self.current_output.push(b),
                _ => {}
            }

            for event in events {
                match event {
                    BoundaryEvent::PromptStart => {
                        self.active = true;
                        self.state = TrackerState::InPrompt;
                    }
                    BoundaryEvent::PromptEnd => {
                        self.state = TrackerState::InInput;
                        self.current_input.clear();
                    }
                    BoundaryEvent::CommandStart => {
                        self.state = TrackerState::InOutput;
                        self.current_output.clear();
                    }
                    BoundaryEvent::CommandDone { exit_code } => {
                        if self.active {
                            match self.state {
                                TrackerState::InOutput => {
                                    // C marker was seen: command and output are separate
                                    self.flush_command(Some(exit_code));
                                }
                                TrackerState::InInput => {
                                    // No C marker: everything between B and D is in current_input.
                                    // Split into command (first line) and output (rest).
                                    self.flush_combined(Some(exit_code));
                                }
                                _ => {}
                            }
                        }
                        self.state = TrackerState::Idle;
                    }
                }
            }
        }
    }

    fn flush_command(&mut self, exit_code: Option<i32>) {
        let command_raw = strip_ansi_escapes::strip(&self.current_input);
        let command = String::from_utf8_lossy(&command_raw).trim().to_string();

        let output_raw = strip_ansi_escapes::strip(&self.current_output);
        let output = String::from_utf8_lossy(&output_raw).trim().to_string();

        if command.is_empty() {
            self.current_input.clear();
            self.current_output.clear();
            return;
        }

        self.write_entry(&command, &output, exit_code);
        self.current_input.clear();
        self.current_output.clear();
    }

    /// Fallback when no C marker was seen: everything between B and D is in current_input.
    /// Split heuristically: the command is echoed, so take lines before the echo as input
    /// and lines after as output. If we can't split, treat it all as combined.
    fn flush_combined(&mut self, exit_code: Option<i32>) {
        let raw = strip_ansi_escapes::strip(&self.current_input);
        let text = String::from_utf8_lossy(&raw).trim().to_string();

        if text.is_empty() {
            self.current_input.clear();
            return;
        }

        // Heuristic: split at the first blank line or use the last non-empty line
        // as the command echo boundary. For simplicity, take the first line as
        // the command and the rest as output.
        let mut lines = text.lines();
        let command = lines.next().unwrap_or("").trim().to_string();
        let output: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();

        if command.is_empty() {
            self.current_input.clear();
            return;
        }

        self.write_entry(&command, &output, exit_code);
        self.current_input.clear();
    }

    fn write_entry(&mut self, command: &str, output: &str, exit_code: Option<i32>) {
        let entry = CommandEntry {
            index: self.command_index,
            command: command.to_string(),
            exit_code,
            output: output.to_string(),
            timestamp: Utc::now(),
        };

        self.command_index += 1;

        if let Ok(json) = serde_json::to_string(&entry) {
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.commands_path)
            {
                let _ = writeln!(f, "{}", json);
            }
        }
    }

    /// Whether OSC 133 markers have been detected (shell integration is active).
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        self.active
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_osc133_a() {
        let mut parser = BoundaryParser::new();
        let events = parser.feed(b"\x1b]133;A\x07");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BoundaryEvent::PromptStart));
    }

    #[test]
    fn parse_osc133_b() {
        let mut parser = BoundaryParser::new();
        let events = parser.feed(b"\x1b]133;B\x07");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BoundaryEvent::PromptEnd));
    }

    #[test]
    fn parse_osc133_c() {
        let mut parser = BoundaryParser::new();
        let events = parser.feed(b"\x1b]133;C\x07");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BoundaryEvent::CommandStart));
    }

    #[test]
    fn parse_osc133_d_with_exit_code() {
        let mut parser = BoundaryParser::new();
        let events = parser.feed(b"\x1b]133;D;42\x07");
        assert_eq!(events.len(), 1);
        match &events[0] {
            BoundaryEvent::CommandDone { exit_code } => assert_eq!(*exit_code, 42),
            _ => panic!("expected CommandDone"),
        }
    }

    #[test]
    fn parse_osc133_d_zero() {
        let mut parser = BoundaryParser::new();
        let events = parser.feed(b"\x1b]133;D;0\x07");
        assert_eq!(events.len(), 1);
        match &events[0] {
            BoundaryEvent::CommandDone { exit_code } => assert_eq!(*exit_code, 0),
            _ => panic!("expected CommandDone"),
        }
    }

    #[test]
    fn parse_multiple_markers() {
        let mut parser = BoundaryParser::new();
        let events = parser.feed(b"hello\x1b]133;A\x07prompt\x1b]133;B\x07input\x1b]133;C\x07output\x1b]133;D;0\x07");
        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], BoundaryEvent::PromptStart));
        assert!(matches!(events[1], BoundaryEvent::PromptEnd));
        assert!(matches!(events[2], BoundaryEvent::CommandStart));
        assert!(matches!(events[3], BoundaryEvent::CommandDone { exit_code: 0 }));
    }

    #[test]
    fn parse_split_across_chunks() {
        let mut parser = BoundaryParser::new();
        // Split the sequence \x1b]133;A\x07 across two chunks
        let e1 = parser.feed(b"\x1b]13");
        assert!(e1.is_empty());
        let e2 = parser.feed(b"3;A\x07");
        assert_eq!(e2.len(), 1);
        assert!(matches!(e2[0], BoundaryEvent::PromptStart));
    }

    #[test]
    fn parse_st_terminator() {
        let mut parser = BoundaryParser::new();
        // Use ST (\x1b\\) instead of BEL
        let events = parser.feed(b"\x1b]133;A\x1b\\");
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], BoundaryEvent::PromptStart));
    }

    #[test]
    fn ignore_non_133_osc() {
        let mut parser = BoundaryParser::new();
        let events = parser.feed(b"\x1b]0;title\x07");
        assert!(events.is_empty());
    }
}
