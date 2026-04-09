# termsnoop.psm1 — Terminal capture for AI CLI integration (PowerShell transcript implementation)

#region Private helpers

function New-TermSnoopId {
    $chars = 'abcdefghijklmnopqrstuvwxyz0123456789'
    -join (1..7 | ForEach-Object { $chars[(Get-Random -Maximum $chars.Length)] })
}

function Get-TermSnoopSessionsDir {
    $base = if ($env:TERMSNOOP_HOME) { $env:TERMSNOOP_HOME } else { Join-Path $HOME '.termsnoop' }
    $dir = Join-Path $base 'sessions'
    if (-not (Test-Path $dir)) {
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
    }
    return $dir
}

function Get-TermSnoopSessionPath {
    param([string]$Id)
    Join-Path (Get-TermSnoopSessionsDir) $Id
}

function Get-AllTermSnoopSessions {
    $sessionsDir = Get-TermSnoopSessionsDir
    $sessions = @()
    if (Test-Path $sessionsDir) {
        Get-ChildItem -Path $sessionsDir -Directory | ForEach-Object {
            $metaPath = Join-Path $_.FullName 'meta.json'
            if (Test-Path $metaPath) {
                try {
                    $meta = Get-Content -Path $metaPath -Raw -Encoding UTF8 | ConvertFrom-Json
                    $sessions += $meta
                } catch {}
            }
        }
    }
    return $sessions | Sort-Object -Property started_at -Descending
}

function Find-LatestTermSnoopSession {
    $sessions = Get-AllTermSnoopSessions
    if ($sessions.Count -eq 0) {
        throw 'No sessions found. Start one with: Start-TermSnoop'
    }
    return $sessions[0].id
}

function Remove-TranscriptMetadata {
    param([string]$Text)
    $lines = $Text -split "`r?`n"
    $filtered = $lines | Where-Object {
        $_ -notmatch '^\*{20,}$' -and
        $_ -notmatch '^Transcript started' -and
        $_ -notmatch '^Transcript ended' -and
        $_ -notmatch '^Start time\s*:' -and
        $_ -notmatch '^End time\s*:' -and
        $_ -notmatch '^Username\s*:' -and
        $_ -notmatch '^RunAs User\s*:' -and
        $_ -notmatch '^Configuration Name\s*:' -and
        $_ -notmatch '^Machine\s*:' -and
        $_ -notmatch '^Host Application\s*:' -and
        $_ -notmatch '^Process ID\s*:' -and
        $_ -notmatch '^PSVersion\s*:' -and
        $_ -notmatch '^PSEdition\s*:' -and
        $_ -notmatch '^PSCompatibleVersions\s*:' -and
        $_ -notmatch '^CLRVersion\s*:' -and
        $_ -notmatch '^BuildVersion\s*:' -and
        $_ -notmatch '^SerializationVersion\s*:' -and
        $_ -notmatch '^Command start time\s*:' -and
        $_ -notmatch '^Windows PowerShell transcript'
    }
    ($filtered -join "`n").Trim()
}

function Remove-AnsiEscapes {
    param([string]$Text)
    $Text -replace '\x1b\[[0-9;]*[a-zA-Z]', '' -replace '\x1b\][^\x07]*\x07', '' -replace '\x1b\].*?\x1b\\', ''
}

#endregion

#region Public functions

function Start-TermSnoop {
    <#
    .SYNOPSIS
        Start a captured terminal session using PowerShell transcript.
    .DESCRIPTION
        Creates a new termsnoop session that captures all terminal I/O via
        Start-Transcript. A prompt hook tracks individual commands into
        commands.jsonl for structured access.
    .PARAMETER Name
        Optional human-friendly session name.
    .EXAMPLE
        Start-TermSnoop
    .EXAMPLE
        Start-TermSnoop -Name 'build'
    #>
    [CmdletBinding()]
    param(
        [Parameter()]
        [string]$Name
    )

    if ($global:__TermSnoopState) {
        Write-Warning "termsnoop session '$($global:__TermSnoopState.Id)' is already active. Run Stop-TermSnoop first."
        return
    }

    $id = New-TermSnoopId
    $sessionDir = Get-TermSnoopSessionPath -Id $id
    New-Item -ItemType Directory -Path $sessionDir -Force | Out-Null

    # Build meta.json with field order matching Rust implementation
    $meta = [ordered]@{
        id         = $id
    }
    if ($Name) {
        $meta['name'] = $Name
    }
    $meta['pid'] = $PID
    $meta['shell'] = 'pwsh'
    $meta['cwd'] = (Get-Location).Path
    $meta['started_at'] = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $meta['status'] = 'active'

    $metaPath = Join-Path $sessionDir 'meta.json'
    ($meta | ConvertTo-Json -Depth 10) | Set-Content -Path $metaPath -Encoding UTF8 -NoNewline

    $logPath = Join-Path $sessionDir 'output.log'

    # Start transcript
    Start-Transcript -Path $logPath -IncludeInvocationHeader | Out-Null

    # Initialize state
    $lastHist = Get-History -Count 1 -ErrorAction SilentlyContinue
    $global:__TermSnoopState = @{
        Id              = $id
        SessionDir      = $sessionDir
        LogPath         = $logPath
        CommandsPath    = Join-Path $sessionDir 'commands.jsonl'
        CommandIndex    = 0
        LastHistoryId   = if ($lastHist) { $lastHist.Id } else { 0 }
        LastLogPosition = if (Test-Path $logPath) { (Get-Item $logPath).Length } else { 0 }
    }

    # Save original prompt and install tracking hook
    $global:__TermSnoopOrigPrompt = (Get-Item Function:\prompt).ScriptBlock

    Set-Item Function:\prompt -Value {
        # Capture $? and $LASTEXITCODE immediately before any statements overwrite them
        $__tsSuccess = $?
        $__tsLastExit = $global:LASTEXITCODE

        try {
            $ts = $global:__TermSnoopState
            if ($ts) {
                $lastHist = Get-History -Count 1 -ErrorAction SilentlyContinue
                if ($lastHist -and $lastHist.Id -gt $ts.LastHistoryId) {
                    # Determine exit code
                    $ec = if ($__tsSuccess) { 0 } else {
                        if ($null -ne $__tsLastExit -and $__tsLastExit -ne 0) { $__tsLastExit } else { 1 }
                    }

                    # Capture output from transcript delta
                    $output = ''
                    try {
                        $fs = [System.IO.File]::Open(
                            $ts.LogPath,
                            [System.IO.FileMode]::Open,
                            [System.IO.FileAccess]::Read,
                            [System.IO.FileShare]::ReadWrite
                        )
                        try {
                            if ($fs.Length -gt $ts.LastLogPosition) {
                                $fs.Position = $ts.LastLogPosition
                                $count = [int]($fs.Length - $ts.LastLogPosition)
                                $bytes = [byte[]]::new($count)
                                $null = $fs.Read($bytes, 0, $bytes.Length)
                                $ts.LastLogPosition = $fs.Length
                                $raw = [System.Text.Encoding]::UTF8.GetString($bytes)

                                # Strip transcript metadata and prompt lines
                                $lines = $raw -split "`r?`n" | Where-Object {
                                    $_ -notmatch '^\*{20,}$' -and
                                    $_ -notmatch '^Command start time\s*:' -and
                                    $_ -notmatch '^Transcript started' -and
                                    $_ -notmatch '^Transcript ended'
                                }
                                # Remove prompt echo lines (PS C:\path>)
                                $lines = $lines | Where-Object { $_ -notmatch '^PS .+>' }
                                $output = ($lines -join "`n").Trim()
                            }
                        } finally {
                            $fs.Close()
                        }
                    } catch {
                        # Output capture is best-effort
                    }

                    # Write command entry to commands.jsonl
                    $entry = [ordered]@{
                        index     = [int]$ts.CommandIndex
                        command   = $lastHist.CommandLine
                        exit_code = [int]$ec
                        output    = $output
                        timestamp = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
                    }
                    $json = ($entry | ConvertTo-Json -Compress)
                    [System.IO.File]::AppendAllText(
                        $ts.CommandsPath,
                        "$json`n",
                        [System.Text.Encoding]::UTF8
                    )

                    $ts.CommandIndex++
                    $ts.LastHistoryId = $lastHist.Id
                }
            }
        } catch {
            # Never let tracking errors break the prompt
        }

        # Restore $LASTEXITCODE which may have been changed by tracking code
        $global:LASTEXITCODE = $__tsLastExit

        # Call original prompt
        try {
            if ($global:__TermSnoopOrigPrompt) {
                & $global:__TermSnoopOrigPrompt
            } else {
                "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
            }
        } catch {
            "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
        }
    }

    $env:TERMSNOOP_SESSION = $id

    Write-Host "`u{1F7E2} Session $id started" -ForegroundColor Green
    Write-Host "   Logging to: $sessionDir"
    Write-Host "   Command tracking: enabled"
    Write-Host "   Run Stop-TermSnoop when done."
}

function Stop-TermSnoop {
    <#
    .SYNOPSIS
        Stop the active termsnoop session.
    .DESCRIPTION
        Stops the transcript, updates session metadata, and restores the
        original prompt function.
    #>
    [CmdletBinding()]
    param()

    if (-not $global:__TermSnoopState) {
        Write-Warning 'No active termsnoop session.'
        return
    }

    $id = $global:__TermSnoopState.Id
    $sessionDir = $global:__TermSnoopState.SessionDir

    # Stop transcript
    try { Stop-Transcript | Out-Null } catch {}

    # Update meta.json
    $metaPath = Join-Path $sessionDir 'meta.json'
    if (Test-Path $metaPath) {
        try {
            $meta = Get-Content -Path $metaPath -Raw -Encoding UTF8 | ConvertFrom-Json -AsHashtable
            $meta['status'] = 'exited'
            $meta['ended_at'] = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
            # Preserve field order
            $ordered = [ordered]@{}
            foreach ($key in @('id', 'name', 'pid', 'shell', 'cwd', 'started_at', 'ended_at', 'status')) {
                if ($meta.ContainsKey($key)) {
                    $ordered[$key] = $meta[$key]
                }
            }
            ($ordered | ConvertTo-Json -Depth 10) | Set-Content -Path $metaPath -Encoding UTF8 -NoNewline
        } catch {
            Write-Warning "Failed to update meta.json: $_"
        }
    }

    # Restore original prompt
    if ($global:__TermSnoopOrigPrompt) {
        Set-Item Function:\prompt -Value $global:__TermSnoopOrigPrompt
        $global:__TermSnoopOrigPrompt = $null
    }

    # Clear state
    $global:__TermSnoopState = $null
    $env:TERMSNOOP_SESSION = $null

    Write-Host "`u{1F534} Session $id ended." -ForegroundColor Red
}

function Get-TermSnoopSession {
    <#
    .SYNOPSIS
        List active and recent termsnoop sessions.
    .DESCRIPTION
        Reads all session metadata from ~/.termsnoop/sessions/ and displays
        a formatted table matching the Rust implementation output.
    #>
    [CmdletBinding()]
    param()

    $sessions = Get-AllTermSnoopSessions

    if ($sessions.Count -eq 0) {
        Write-Host 'No sessions found.'
        return
    }

    $header = '{0,-10} {1,-15} {2,-10} {3,-8} {4}' -f 'ID', 'Name', 'Status', 'Shell', 'Started'
    Write-Host $header
    Write-Host ('-' * 70)
    foreach ($s in $sessions) {
        $name = if ($s.name) { $s.name } else { '-' }
        $started = try {
            ([datetime]::Parse($s.started_at)).ToString('yyyy-MM-dd HH:mm:ss')
        } catch { $s.started_at }
        $line = '{0,-10} {1,-15} {2,-10} {3,-8} {4}' -f $s.id, $name, $s.status, $s.shell, $started
        Write-Host $line
    }
}

function Read-TermSnoopSession {
    <#
    .SYNOPSIS
        Read output from a termsnoop session.
    .DESCRIPTION
        Reads terminal output from a session's log files. Supports reading
        structured command entries or raw output lines.
    .PARAMETER SessionId
        Session ID to read. If omitted, reads the latest session.
    .PARAMETER LastCommands
        Number of recent commands to return (from commands.jsonl).
    .PARAMETER Tail
        Number of lines to return from the end of output.log.
    .PARAMETER Json
        Output as structured JSON instead of plain text.
    #>
    [CmdletBinding()]
    param(
        [Parameter(Position = 0)]
        [string]$SessionId,

        [Parameter()]
        [int]$LastCommands,

        [Parameter()]
        [int]$Tail,

        [Parameter()]
        [switch]$Json
    )

    if (-not $SessionId) {
        $SessionId = Find-LatestTermSnoopSession
    }

    $sessionDir = Get-TermSnoopSessionPath -Id $SessionId
    if (-not (Test-Path $sessionDir)) {
        throw "Session '$SessionId' not found."
    }

    # --last-commands mode
    if ($LastCommands -gt 0) {
        $commandsPath = Join-Path $sessionDir 'commands.jsonl'
        if (-not (Test-Path $commandsPath)) {
            throw "No structured command data. Command boundary detection may not be active.`nUse -Tail instead to read raw output."
        }

        $entries = @(Get-Content -Path $commandsPath -Encoding UTF8 |
            Where-Object { $_.Trim() } |
            ForEach-Object {
                try { $_ | ConvertFrom-Json } catch {}
            })

        $start = [Math]::Max(0, $entries.Count - $LastCommands)
        $slice = @($entries[$start..($entries.Count - 1)])

        if ($Json) {
            ConvertTo-Json -InputObject $slice -Depth 10
        } else {
            foreach ($e in $slice) {
                Write-Host "$ $($e.command)"
                if ($e.output) {
                    Write-Host $e.output
                }
                Write-Host ''
            }
        }
        return
    }

    # --tail mode or full output
    $logPath = Join-Path $sessionDir 'output.log'
    if (-not (Test-Path $logPath)) {
        throw "No output log for session '$SessionId'."
    }

    # Read with sharing (file may be locked by active transcript)
    $text = try {
        $fs = [System.IO.File]::Open(
            $logPath,
            [System.IO.FileMode]::Open,
            [System.IO.FileAccess]::Read,
            [System.IO.FileShare]::ReadWrite
        )
        try {
            $reader = [System.IO.StreamReader]::new($fs, [System.Text.Encoding]::UTF8)
            $reader.ReadToEnd()
        } finally {
            $fs.Close()
        }
    } catch {
        Get-Content -Path $logPath -Raw -Encoding UTF8
    }

    # Strip ANSI escapes (for Rust-created sessions) and transcript metadata
    $text = Remove-AnsiEscapes -Text $text
    $text = Remove-TranscriptMetadata -Text $text

    if ($Tail -gt 0) {
        $lines = @($text -split "`r?`n")
        $start = [Math]::Max(0, $lines.Count - $Tail)
        $text = ($lines[$start..($lines.Count - 1)] -join "`n")
    }

    if ($Json) {
        $result = [ordered]@{
            session_id = $SessionId
            lines      = @($text -split "`r?`n").Count
            output     = $text
        }
        ConvertTo-Json -InputObject $result -Depth 10
    } else {
        Write-Output $text
    }
}

function Clear-TermSnoopSession {
    <#
    .SYNOPSIS
        Remove old termsnoop session data.
    .DESCRIPTION
        Deletes session directories older than the specified number of days.
    .PARAMETER Days
        Delete sessions older than this many days. Default: 7.
    #>
    [CmdletBinding()]
    param(
        [Parameter()]
        [int]$Days = 7
    )

    $cutoff = (Get-Date).AddDays(-$Days)
    $cleaned = 0

    foreach ($session in Get-AllTermSnoopSessions) {
        try {
            $startedAt = [datetime]::Parse($session.started_at).ToUniversalTime()
            if ($startedAt -lt $cutoff) {
                $dir = Get-TermSnoopSessionPath -Id $session.id
                if (Test-Path $dir) {
                    Remove-Item -Path $dir -Recurse -Force
                    $cleaned++
                }
            }
        } catch {}
    }

    Write-Host "Cleaned $cleaned session(s) older than $Days days."
}

#endregion

Export-ModuleMember -Function @(
    'Start-TermSnoop'
    'Stop-TermSnoop'
    'Get-TermSnoopSession'
    'Read-TermSnoopSession'
    'Clear-TermSnoopSession'
)
