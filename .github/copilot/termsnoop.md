---
name: termsnoop
description: Read terminal output from a user's other terminal sessions
triggers:
  - "check my terminal"
  - "what error"
  - "terminal output"
  - "what happened in my terminal"
  - "read my terminal"
  - "look at my other terminal"
---

# termsnoop — Terminal Output Reader

When the user asks you to check their terminal output, look at errors from
another terminal, or reference output from commands they ran elsewhere:

1. Run `termsnoop list` to see active sessions
2. Run `termsnoop read --last-commands 3 --json` to get recent command output
3. If `--last-commands` fails (no command tracking), fall back to `termsnoop read --tail 50`
4. Use the output to help the user diagnose and fix issues

## Prerequisites

The user must have `termsnoop` running in their other terminal. They start it with:
```
termsnoop start
```

## Commands

| Command | Description |
|---------|-------------|
| `termsnoop list` | List active/recent sessions |
| `termsnoop read --last-commands N --json` | Get last N commands with output (structured) |
| `termsnoop read --tail N` | Get last N lines of raw output |
| `termsnoop read [session-id] --json` | Read specific session |

## MCP Server

termsnoop can also be used as an MCP server for Claude CLI and other MCP clients:

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
