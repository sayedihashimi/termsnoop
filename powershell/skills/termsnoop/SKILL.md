---
name: termsnoop
description: Read terminal output from a user's other terminal sessions. Use when the user asks to check their terminal, look at errors, or reference output from commands they ran elsewhere.
---

# termsnoop — Terminal Output Reader (PowerShell)

When the user asks you to check their terminal output, look at errors from
another terminal, or reference output from commands they ran elsewhere:

1. Run `Import-Module termsnoop -ErrorAction SilentlyContinue` to load the module
2. Run `Get-TermSnoopSession` to see active sessions
3. Run `Read-TermSnoopSession -LastCommands 3 -Json` to get recent command output
4. If `-LastCommands` fails (no command tracking), fall back to `Read-TermSnoopSession -Tail 50`
5. Use the output to help the user diagnose and fix issues

## Prerequisites

The user must have the termsnoop module loaded and a session started in their
other terminal:

```powershell
Import-Module termsnoop
Start-TermSnoop
```

## Commands

| Command | Description |
|---------|-------------|
| `Get-TermSnoopSession` | List active/recent sessions |
| `Read-TermSnoopSession -LastCommands N -Json` | Get last N commands with output (structured) |
| `Read-TermSnoopSession -Tail N` | Get last N lines of raw output |
| `Read-TermSnoopSession -SessionId <id> -Json` | Read specific session |

## Notes

- This is the PowerShell transcript implementation (no PTY proxy)
- Sessions are stored at `~/.termsnoop/sessions/` in the same format as the
  Rust implementation — both are interoperable
- The module must be imported before using the commands
