# termsnoop

**Terminal capture for AI CLI integration.**

`termsnoop` captures terminal output so AI tools (Copilot CLI, Claude CLI, etc.)
can read it without the user needing to copy/paste.

## Implementations

| Implementation | Directory | Description |
|---------------|-----------|-------------|
| **Rust (PTY proxy)** | [`rust/`](rust/) | Full PTY proxy that spawns a captured shell session. Cross-platform, supports interactive programs. |
| **PowerShell (transcript)** | [`powershell/`](powershell/) | Lightweight approach using PowerShell's `Start-Transcript`. PowerShell-only, simpler setup. |

## How It Works

```
Terminal A (AI CLI)                Terminal B (User's work)
┌──────────────────────┐          ┌──────────────────────────────┐
│  copilot cli         │          │  $ termsnoop                 │
│                      │          │  () Session abc1234 started  │
│  > "check terminal"  │          │  $ npm run build             │
│                      │          │  ERROR: Module not found     │
│  [reads termsnoop    │◄─────────│                              │
│   session output]    │  file    │  (output logged to           │
│                      │  based   │   ~/.termsnoop/sessions/)    │
│  "I see the error,   │          │                              │
│   let me fix it..."  │          │                              │
└──────────────────────┘          └──────────────────────────────┘
```

## Installation

### Rust implementation (from source)

```bash
cd rust
cargo install --path .
```

### From GitHub Releases

Download the latest binary for your platform from
[Releases](../../releases) and add it to your `PATH`.

## Quick Start

**1. Start a captured session:**

```bash
termsnoop start
```

This spawns your default shell inside a PTY proxy. Everything you type and see
is logged. Use the terminal normally — run builds, tests, whatever.

**2. From another terminal, read the output:**

```bash
# List sessions
termsnoop list

# Read last 3 commands with structured output
termsnoop read --last-commands 3 --json

# Read last 50 lines of raw output
termsnoop read --tail 50

# Exit the captured session when done
exit
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `termsnoop` | Start a captured shell session (alias for `start`) |
| `termsnoop start` | Start a captured shell session |
| `termsnoop start --name build` | Start with a human-friendly name |
| `termsnoop start --shell bash` | Start with a specific shell |
| `termsnoop list` | List active and recent sessions |
| `termsnoop read [session-id]` | Read output (latest session if omitted) |
| `termsnoop read --last-commands N` | Read last N commands' output |
| `termsnoop read --tail N` | Read last N lines |
| `termsnoop read --json` | Structured JSON output |
| `termsnoop clean` | Remove sessions older than 7 days |
| `termsnoop clean --days 30` | Remove sessions older than 30 days |
| `termsnoop mcp-server` | Run as MCP server over stdio |

## AI Integration

### MCP Server (Claude CLI, etc.)

termsnoop includes a built-in MCP server. Add it to your Claude CLI config:

```json
{
  "mcpServers": {
    "termsnoop": {
      "command": "termsnoop",
      "args": ["mcp-server"]
    }
  }
}
```

The MCP server exposes two tools:

- **`read_terminal_output`** — Read recent output from a session
  - `session_id` (optional) — specific session, or latest
  - `last_commands` (optional) — last N commands with output
  - `tail_lines` (optional) — last N lines of raw output
- **`list_terminal_sessions`** — List all sessions

### Copilot CLI Skill

A skill file is included at `.github/skills/termsnoop/SKILL.md`. When present in a
repository, Copilot CLI will automatically invoke termsnoop when you ask it to
check your terminal, look at errors, or reference output from another terminal.

## Session Storage

Sessions are stored at `~/.termsnoop/sessions/<session-id>/`:

```
~/.termsnoop/sessions/abc1234/
├── meta.json          # Session metadata (ID, shell, status, timestamps)
├── output.log         # Raw captured output (with ANSI codes)
└── commands.jsonl     # Structured command+output pairs (when available)
```

### Command Tracking

termsnoop injects shell integration scripts (OSC 133 markers) to detect
command boundaries. This provides structured command+output data in
`commands.jsonl`. Supported shells:

| Shell | Status |
|-------|--------|
| pwsh (PowerShell Core) | ✅ Full support |
| bash | ✅ Full support |
| zsh | ✅ Full support |
| fish | ✅ Full support |
| Other | ⚠️ Graceful degradation (`--tail` still works) |

## Configuration

Create `~/.termsnoop/config.toml` to customize defaults:

```toml
# Maximum age of sessions before `clean` removes them (days)
session_ttl_days = 7

# Maximum log file size per session (bytes, default 50 MB)
max_log_bytes = 52428800

# Number of commands to keep in shell history (default 500)
command_history_size = 500

# Default shell to spawn (if not specified on command line)
# default_shell = "pwsh"
```

## Architecture

termsnoop is built in Rust with three layers:

1. **Core CLI** — PTY proxy via `portable-pty` (ConPTY on Windows, PTY on Unix),
   session management, ANSI stripping, command boundary detection
2. **MCP Server** — Model Context Protocol server over stdio via `rmcp`,
   exposing terminal reading tools for AI integration
3. **Copilot Skill** — YAML/markdown config teaching Copilot CLI when to
   invoke termsnoop

## Privacy & Security

- **Opt-in only**: Only sessions explicitly started with `termsnoop` are captured.
  Other terminals are never accessed.
- **Local storage**: Session data is stored locally with user-only permissions.
  No network communication.
- **Auto-cleanup**: Old sessions are removed with `termsnoop clean`.
- **Full PTY**: Interactive programs (vim, less, htop) work normally inside
  a termsnoop session.

## Building from Source

### Rust

```bash
git clone https://github.com/user/termsnoop.git
cd termsnoop/rust

cargo build --release
cargo test
cargo install --path .
```

## License

MIT — see [LICENSE](LICENSE) for details.
