# termsnoop — PowerShell Transcript Implementation

A lightweight implementation of termsnoop using PowerShell's built-in
`Start-Transcript` / `Stop-Transcript` for terminal capture.

## Status

🚧 Under development

## Approach

Instead of a PTY proxy, this implementation uses PowerShell's native transcript
feature to capture all terminal I/O to a session-scoped log file. AI tools can
then read the transcript file to access recent terminal output.

### Trade-offs vs Rust (PTY proxy)

| Aspect | PowerShell Transcript | Rust PTY Proxy |
|--------|----------------------|----------------|
| Setup complexity | Minimal — uses built-in cmdlets | Requires compiled binary |
| Shell support | PowerShell only | Any shell (bash, zsh, fish, pwsh, etc.) |
| Cross-platform | Where PowerShell runs | Full cross-platform via ConPTY/PTY |
| Interactive programs | Limited capture | Full PTY — vim, less, htop all work |
| Terminal behavior | No interference | PTY proxy may affect some terminal features |
| Dependencies | None (built-in) | Rust toolchain + crates |
