#!/usr/bin/env pwsh
<#
.SYNOPSIS
    CLI entry point for termsnoop (PowerShell transcript implementation).
.DESCRIPTION
    Provides a command-line interface matching the Rust termsnoop binary.
    Wraps the termsnoop PowerShell module functions.
.EXAMPLE
    termsnoop-cli.ps1 start --name build
    termsnoop-cli.ps1 list
    termsnoop-cli.ps1 read --last-commands 3 --json
    termsnoop-cli.ps1 stop
    termsnoop-cli.ps1 clean --days 30
#>

$ErrorActionPreference = 'Stop'

# Import module from same directory
Import-Module (Join-Path $PSScriptRoot 'termsnoop.psd1') -Force

$subcommand = if ($args.Count -gt 0) { $args[0] } else { $null }
$rest = if ($args.Count -gt 1) { $args[1..($args.Count - 1)] } else { @() }

switch ($subcommand) {
    'start' {
        $params = @{}
        for ($i = 0; $i -lt $rest.Count; $i++) {
            if ($rest[$i] -eq '--name' -and ($i + 1) -lt $rest.Count) {
                $params['Name'] = $rest[$i + 1]
                $i++
            }
        }
        Start-TermSnoop @params
    }

    'stop' {
        Stop-TermSnoop
    }

    'list' {
        Get-TermSnoopSession
    }

    'read' {
        $params = @{}
        for ($i = 0; $i -lt $rest.Count; $i++) {
            switch ($rest[$i]) {
                '--last-commands' {
                    $i++
                    $params['LastCommands'] = [int]$rest[$i]
                }
                '--tail' {
                    $i++
                    $params['Tail'] = [int]$rest[$i]
                }
                '--json' {
                    $params['Json'] = $true
                }
                default {
                    if (-not $rest[$i].StartsWith('-')) {
                        $params['SessionId'] = $rest[$i]
                    }
                }
            }
        }
        Read-TermSnoopSession @params
    }

    'clean' {
        $params = @{}
        for ($i = 0; $i -lt $rest.Count; $i++) {
            if ($rest[$i] -eq '--days' -and ($i + 1) -lt $rest.Count) {
                $params['Days'] = [int]$rest[$i + 1]
                $i++
            }
        }
        Clear-TermSnoopSession @params
    }

    default {
        if ($subcommand) {
            Write-Error "Unknown command: $subcommand"
        }
        Write-Host 'termsnoop (PowerShell transcript implementation)'
        Write-Host ''
        Write-Host 'Usage: termsnoop-cli.ps1 <command> [options]'
        Write-Host ''
        Write-Host 'Commands:'
        Write-Host '  start [--name <name>]                                Start a captured session'
        Write-Host '  stop                                                 Stop the active session'
        Write-Host '  list                                                 List sessions'
        Write-Host '  read [session-id] [--last-commands N] [--tail N] [--json]  Read session output'
        Write-Host '  clean [--days N]                                     Remove old sessions (default: 7 days)'
    }
}
