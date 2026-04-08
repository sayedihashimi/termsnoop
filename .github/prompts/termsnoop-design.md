# termsnoop — Terminal Capture for AI CLI Integration

## Problem
When using AI CLI tools (Copilot CLI, Claude CLI), users frequently need to run commands in a separate terminal, then manually copy/paste output back to the AI. This is tedious, error-prone, and breaks flow.

## Proposed Solution
`termsnoop` — a cross-platform CLI tool (Rust) that creates a captured shell session via PTY proxy. All terminal I/O is logged to a session-scoped file. AI tools can invoke `termsnoop read` to retrieve recent output without the user copy/pasting anything.

## Architecture

### Three Layers

1. **Core CLI** (`termsnoop`) — Rust binary
   - PTY proxy that spawns user's default shell
   - Session management (create, list, read, clean)
   - Output capture with ANSI stripping
   - Command boundary detection
   - Cross-platform: ConPTY (Windows), PTY (macOS/Linux)

2. **MCP Server** (`termsnoop mcp-server`) — built-in mode
   - Model Context Protocol server over stdio
   - Exposes `read_terminal`, `list_sessions` tools
   - For Claude CLI and any MCP-compatible AI tool

3. **Copilot CLI Skill** — YAML/markdown config
   - Teaches Copilot CLI when/how to invoke termsnoop
   - Triggers on phrases like "check my terminal", "what error"

### How It Works

```
Terminal A (Copilot CLI)          Terminal B (User's work)
┌─────────────────────┐          ┌──────────────────────────┐
│  copilot cli         │          │  $ termsnoop               │
│                      │          │  🟢 Session abc123 started│
│  > "check terminal"  │          │  $ npm run build          │
│                      │          │  ERROR: Module not found  │
│  [invokes termsnoop    │◄────────►│                          │
│   read abc123]       │  file    │  (output logged to       │
│                      │  based   │   ~/.termsnoop/sessions/)  │
│  "I see the error,   │          │                          │
│   let me fix it..."  │          │                          │
└─────────────────────┘          └──────────────────────────┘
```

## CLI Commands

| Command | Description |
|---------|-------------|
| `termsnoop` / `termsnoop start` | Start a captured shell session |
| `termsnoop start --name "build"` | Start with a human-friendly name |
| `termsnoop list` | List active/recent sessions |
| `termsnoop read [session-id]` | Read output (latest session if omitted) |
| `termsnoop read --last-commands N` | Read last N commands' output |
| `termsnoop read --tail N` | Read last N lines |
| `termsnoop read --json` | Structured JSON output (for AI consumption) |
| `termsnoop clean` | Remove old session data |
| `termsnoop mcp-server` | Run as MCP server (stdio) |

## Session Storage

```
~/.termsnoop/sessions/<session-id>/
├── meta.json          # Session metadata
├── output.log         # Raw captured output (with ANSI codes)
└── commands.jsonl     # Structured command+output pairs
```

### meta.json
```json
{
  "id": "abc123",
  "name": "build",
  "pid": 12345,
  "shell": "pwsh",
  "cwd": "C:\\projects\\my-app",
  "started_at": "2026-04-08T02:30:00Z",
  "status": "active"
}
```

### commands.jsonl
```json
{"index": 0, "command": "npm run build", "exit_code": 1, "output": "ERROR: Module not found...", "timestamp": "2026-04-08T02:31:00Z"}
{"index": 1, "command": "ls -la", "exit_code": 0, "output": "total 48\ndrwxr-xr-x ...", "timestamp": "2026-04-08T02:31:15Z"}
```

## Command Boundary Detection

Detecting where one command ends and another begins is non-trivial. Approaches (in priority order):

1. **Shell integration markers** — Set environment variables that inject invisible markers into the prompt (e.g., OSC 133 sequences: `\x1b]133;A\x07` for prompt start, `\x1b]133;C\x07` for command start). Works with modern terminals.

2. **PROMPT_COMMAND / precmd hooks** — When spawning the shell, inject a hook that writes a delimiter to a sidecar file before/after each command. Less invasive than modifying PS1.

3. **Prompt pattern detection** — Regex-based heuristic to detect common prompt patterns (`$`, `>`, `PS C:\>`). Fallback when hooks aren't available.

4. **Graceful degradation** — If boundary detection fails, `termsnoop read --tail N` still works (just raw lines, no command structure).

## Rust Crates (Key Dependencies)

| Crate | Purpose |
|-------|---------|
| `portable-pty` | Cross-platform PTY (ConPTY + Unix) |
| `tokio` | Async I/O for PTY read/write |
| `clap` | CLI argument parsing |
| `strip-ansi-escapes` | Clean ANSI codes from output |
| `serde` / `serde_json` | Structured data serialization |
| `uuid` | Session ID generation |
| `dirs` | Cross-platform home directory |
| `rmcp` or `mcp-rs` | MCP server implementation |

## MCP Server Design

When run as `termsnoop mcp-server`, exposes:

### Tools
- **`read_terminal_output`** — Read recent output from a session
  - Params: `session_id` (optional), `last_commands` (int), `tail_lines` (int), `format` (text/json)
- **`list_terminal_sessions`** — List active sessions
  - Returns: session IDs, names, status, shell, start time

### Integration
```json
// Claude CLI config (~/.claude/config.json or similar)
{
  "mcpServers": {
    "termsnoop": {
      "command": "termsnoop",
      "args": ["mcp-server"]
    }
  }
}
```

## Copilot CLI Skill

A skill definition file that teaches Copilot when to use termsnoop:

```yaml
# ~/.copilot/skills/termsnoop.md or project .github/copilot/skills/termsnoop.md
---
name: termsnoop
description: Read terminal output from a user's other terminal sessions
triggers:
  - "check my terminal"
  - "what error"
  - "terminal output"
  - "what happened in my terminal"
---
# termsnoop - Terminal Output Reader

When the user asks you to check their terminal output, look at errors from
another terminal, or reference output from commands they ran elsewhere:

1. Run `termsnoop list` to see active sessions
2. Run `termsnoop read --last-commands 3 --json` to get recent command output
3. Use the output to help the user diagnose and fix issues

The user has termsnoop running in their other terminal. You can read its output
without the user needing to copy/paste.
```

## Implementation Phases

### Phase 1 — Core PTY Proxy
- [ ] Project setup (Cargo workspace)
- [ ] PTY spawning with `portable-pty`
- [ ] I/O passthrough (stdin→PTY, PTY→stdout+log)
- [ ] Session file management (meta.json, output.log)
- [ ] `termsnoop start` command
- [ ] `termsnoop list` command
- [ ] `termsnoop read` with `--tail` support
- [ ] ANSI stripping for clean read output
- [ ] Cross-platform testing (Windows + macOS/Linux)

### Phase 2 — Command Boundaries
- [ ] Shell integration marker injection (OSC 133)
- [ ] PROMPT_COMMAND / precmd hook injection
- [ ] commands.jsonl structured logging
- [ ] `termsnoop read --last-commands N`
- [ ] Graceful degradation when detection unavailable

### Phase 3 — AI Integration
- [ ] MCP server mode (`termsnoop mcp-server`)
- [ ] `read_terminal_output` tool
- [ ] `list_terminal_sessions` tool
- [ ] Copilot CLI skill definition
- [ ] Documentation and examples

### Phase 4 — Polish & Distribution
- [ ] Homebrew formula / Scoop manifest / cargo install
- [ ] CI/CD for cross-platform releases
- [ ] Session cleanup / TTL for old sessions
- [ ] Config file support (~/.termsnoop/config.toml)
- [ ] README with demos/screenshots

## Design Considerations

- **Privacy**: Only captures sessions explicitly started with `termsnoop`. Never accesses other terminals.
- **Performance**: Async I/O ensures no lag in the terminal experience. Log writes are buffered.
- **Disk usage**: Auto-cleanup of sessions older than N days. Configurable max log size.
- **Security**: Session files are user-readable only (0600 permissions). No network communication.
- **Interactive commands**: Full PTY means vim, less, htop etc. all work. ANSI stripping handles cleanup for AI readability.
