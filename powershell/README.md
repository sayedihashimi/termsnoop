# termsnoop — PowerShell Transcript Implementation

A lightweight implementation of termsnoop using PowerShell's built-in
`Start-Transcript` / `Stop-Transcript` for terminal capture. No compiled
binary needed — pure PowerShell.

## How It Works

Instead of a PTY proxy, this runs **inside** your existing PowerShell session.
`Start-Transcript` captures all terminal I/O, and a prompt hook tracks
individual commands with exit codes into structured JSON.

```
Terminal A (AI CLI)                Terminal B (Your PowerShell)
┌──────────────────────┐          ┌──────────────────────────────┐
│  copilot cli         │          │  PS> Import-Module termsnoop │
│                      │          │  PS> Start-TermSnoop         │
│  > "check terminal"  │          │  🟢 Session abc1234 started  │
│                      │          │  PS> npm run build           │
│  [reads termsnoop    │◄─────────│  ERROR: Module not found     │
│   session output]    │  file    │                              │
│                      │  based   │  (transcript logged to       │
│  "I see the error,   │          │   ~/.termsnoop/sessions/)    │
│   let me fix it..."  │          │                              │
└──────────────────────┘          └──────────────────────────────┘
```

## Installation

### Import from source

```powershell
Import-Module ./powershell/termsnoop.psd1
```

### Add to your PowerShell profile

```powershell
# Add to $PROFILE to auto-load on every session
Import-Module /path/to/termsnoop/powershell/termsnoop.psd1
```

## Quick Start

**1. Start a captured session:**

```powershell
Import-Module termsnoop
Start-TermSnoop
```

Your existing terminal keeps working normally — no new shell is spawned.
All commands and output are logged in the background.

**2. From another terminal (or AI tool), read the output:**

```powershell
Import-Module termsnoop

# List sessions
Get-TermSnoopSession

# Read last 3 commands with structured output
Read-TermSnoopSession -LastCommands 3 -Json

# Read last 50 lines of raw output
Read-TermSnoopSession -Tail 50
```

**3. Stop when done:**

```powershell
Stop-TermSnoop
```

## Command Reference

| PowerShell Function | CLI Equivalent | Description |
|-------------------|----------------|-------------|
| `Start-TermSnoop [-Name <n>]` | `termsnoop-cli.ps1 start` | Start a captured session |
| `Stop-TermSnoop` | `termsnoop-cli.ps1 stop` | Stop the active session |
| `Get-TermSnoopSession` | `termsnoop-cli.ps1 list` | List all sessions |
| `Read-TermSnoopSession [-SessionId <id>] [-LastCommands N] [-Tail N] [-Json]` | `termsnoop-cli.ps1 read` | Read session output |
| `Clear-TermSnoopSession [-Days N]` | `termsnoop-cli.ps1 clean` | Remove old sessions |

## CLI Wrapper

A CLI script is provided for terminal-style usage:

```
./termsnoop-cli.ps1 start --name build
./termsnoop-cli.ps1 list
./termsnoop-cli.ps1 read --last-commands 3 --json
./termsnoop-cli.ps1 stop
./termsnoop-cli.ps1 clean --days 30
```

## Copilot CLI Skill

A skill file is included at `skills/termsnoop/SKILL.md`. Copy it to your
project's `.github/skills/termsnoop/` directory to enable Copilot CLI
integration. Skills must be in their own dedicated subfolder.

## Session Storage

Sessions are stored at `~/.termsnoop/sessions/<session-id>/` in the **same
format** as the Rust implementation:

```
~/.termsnoop/sessions/abc1234/
├── meta.json          # Session metadata
├── output.log         # Transcript output
└── commands.jsonl     # Structured command+output pairs
```

Both implementations can read each other's sessions.

## Trade-offs vs Rust (PTY proxy)

| Aspect | PowerShell Transcript | Rust PTY Proxy |
|--------|----------------------|----------------|
| Setup complexity | Minimal — pure PowerShell | Requires compiled binary |
| Shell support | PowerShell only | Any shell (bash, zsh, fish, pwsh) |
| Cross-platform | Where PowerShell 7+ runs | Full cross-platform via ConPTY/PTY |
| Interactive programs | No interference | Full PTY — vim, less, htop work |
| Terminal behavior | Zero impact | PTY proxy may affect some features |
| Dependencies | None (built-in) | Rust toolchain + crates |
| Command tracking | Get-History + prompt hook | OSC 133 shell integration markers |

## Requirements

- PowerShell 7.0+ (pwsh)

## Running Tests

```powershell
Install-Module Pester -Force -SkipPublisherCheck -MinimumVersion 5.0
Import-Module Pester -MinimumVersion 5.0
Invoke-Pester -Path ./tests/termsnoop.tests.ps1 -Output Detailed
```
