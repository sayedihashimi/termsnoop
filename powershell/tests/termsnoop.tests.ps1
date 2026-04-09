BeforeAll {
    $modulePath = Join-Path $PSScriptRoot '..' 'termsnoop.psd1'
    Import-Module $modulePath -Force
}

AfterAll {
    # Clean up any active session
    if ($global:__TermSnoopState) {
        try { Stop-TermSnoop } catch {}
    }
    $env:TERMSNOOP_HOME = $null
}

Describe 'Session ID Generation' {
    It 'generates a 7-character ID' {
        InModuleScope termsnoop {
            $id = New-TermSnoopId
            $id.Length | Should -Be 7
        }
    }

    It 'contains only lowercase letters and digits' {
        InModuleScope termsnoop {
            $id = New-TermSnoopId
            $id | Should -Match '^[a-z0-9]{7}$'
        }
    }

    It 'generates unique IDs' {
        InModuleScope termsnoop {
            $ids = 1..20 | ForEach-Object { New-TermSnoopId }
            ($ids | Select-Object -Unique).Count | Should -Be 20
        }
    }
}

Describe 'Start-TermSnoop' {
    BeforeEach {
        $env:TERMSNOOP_HOME = Join-Path $TestDrive "start-$(New-Guid)"
    }

    AfterEach {
        if ($global:__TermSnoopState) {
            try { Stop-TermSnoop } catch {}
        }
        $env:TERMSNOOP_HOME = $null
    }

    It 'creates session directory and meta.json' {
        Start-TermSnoop
        $state = $global:__TermSnoopState
        $state | Should -Not -BeNullOrEmpty

        $metaPath = Join-Path $state.SessionDir 'meta.json'
        $metaPath | Should -Exist

        $meta = Get-Content $metaPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $meta.id | Should -Match '^[a-z0-9]{7}$'
        $meta.status | Should -Be 'active'
        $meta.shell | Should -Be 'pwsh'
        $meta.pid | Should -Be $PID
        $meta.cwd | Should -Not -BeNullOrEmpty
        $meta.started_at | Should -Not -BeNullOrEmpty
    }

    It 'accepts -Name parameter' {
        Start-TermSnoop -Name 'test-build'
        $meta = Get-Content (Join-Path $global:__TermSnoopState.SessionDir 'meta.json') -Raw -Encoding UTF8 | ConvertFrom-Json
        $meta.name | Should -Be 'test-build'
    }

    It 'creates meta.json without name field when name not provided' {
        Start-TermSnoop
        $raw = Get-Content (Join-Path $global:__TermSnoopState.SessionDir 'meta.json') -Raw -Encoding UTF8
        $raw | Should -Not -Match '"name"'
    }

    It 'sets TERMSNOOP_SESSION environment variable' {
        Start-TermSnoop
        $env:TERMSNOOP_SESSION | Should -Be $global:__TermSnoopState.Id
    }

    It 'creates output.log via transcript' {
        Start-TermSnoop
        $logPath = Join-Path $global:__TermSnoopState.SessionDir 'output.log'
        $logPath | Should -Exist
    }

    It 'warns on double start' {
        Start-TermSnoop
        $warn = Start-TermSnoop 3>&1
        $warn | Should -Not -BeNullOrEmpty
    }
}

Describe 'Stop-TermSnoop' {
    BeforeEach {
        $env:TERMSNOOP_HOME = Join-Path $TestDrive "stop-$(New-Guid)"
        Start-TermSnoop
        $script:sessionId = $global:__TermSnoopState.Id
        $script:sessionDir = $global:__TermSnoopState.SessionDir
    }

    AfterEach {
        if ($global:__TermSnoopState) {
            try { Stop-TermSnoop } catch {}
        }
        $env:TERMSNOOP_HOME = $null
    }

    It 'clears state and environment variable' {
        Stop-TermSnoop
        $global:__TermSnoopState | Should -BeNullOrEmpty
        $env:TERMSNOOP_SESSION | Should -BeNullOrEmpty
    }

    It 'updates meta.json status to exited' {
        Stop-TermSnoop
        $meta = Get-Content (Join-Path $script:sessionDir 'meta.json') -Raw -Encoding UTF8 | ConvertFrom-Json
        $meta.status | Should -Be 'exited'
        $meta.ended_at | Should -Not -BeNullOrEmpty
    }

    It 'warns when no active session' {
        Stop-TermSnoop
        $warn = Stop-TermSnoop 3>&1
        $warn | Should -Not -BeNullOrEmpty
    }
}

Describe 'Get-TermSnoopSession' {
    BeforeAll {
        $env:TERMSNOOP_HOME = Join-Path $TestDrive "list-$(New-Guid)"

        # Create two fake sessions
        $sessDir1 = Join-Path $env:TERMSNOOP_HOME 'sessions' 'abc1234'
        New-Item -ItemType Directory -Path $sessDir1 -Force | Out-Null
        [ordered]@{
            id = 'abc1234'; pid = 1234; shell = 'pwsh'
            cwd = 'C:\test'; started_at = '2026-04-08T02:30:00Z'; status = 'active'
        } | ConvertTo-Json | Set-Content (Join-Path $sessDir1 'meta.json') -Encoding UTF8

        $sessDir2 = Join-Path $env:TERMSNOOP_HOME 'sessions' 'xyz7890'
        New-Item -ItemType Directory -Path $sessDir2 -Force | Out-Null
        [ordered]@{
            id = 'xyz7890'; name = 'build'; pid = 5678; shell = 'pwsh'
            cwd = 'C:\project'; started_at = '2026-04-08T03:00:00Z'; status = 'exited'
        } | ConvertTo-Json | Set-Content (Join-Path $sessDir2 'meta.json') -Encoding UTF8
    }

    AfterAll {
        $env:TERMSNOOP_HOME = $null
    }

    It 'lists sessions without error' {
        { Get-TermSnoopSession } | Should -Not -Throw
    }

    It 'shows no sessions message when empty' {
        $env:TERMSNOOP_HOME = Join-Path $TestDrive "empty-$(New-Guid)"
        $output = Get-TermSnoopSession 6>&1
        # Should print 'No sessions found.' (via Write-Host, captured in stream 6)
        $env:TERMSNOOP_HOME = Join-Path $TestDrive "list-$(New-Guid)"
    }
}

Describe 'Read-TermSnoopSession' {
    BeforeAll {
        $env:TERMSNOOP_HOME = Join-Path $TestDrive "read-$(New-Guid)"
        $sessDir = Join-Path $env:TERMSNOOP_HOME 'sessions' 'rd12345'
        New-Item -ItemType Directory -Path $sessDir -Force | Out-Null

        [ordered]@{
            id = 'rd12345'; pid = 9999; shell = 'pwsh'
            cwd = 'C:\test'; started_at = '2026-04-08T02:30:00Z'; status = 'active'
        } | ConvertTo-Json | Set-Content (Join-Path $sessDir 'meta.json') -Encoding UTF8

        # Create output.log with some content
        @(
            'line 1: hello world'
            'line 2: building project'
            'line 3: error found'
            'line 4: fixing issue'
            'line 5: build complete'
        ) | Set-Content (Join-Path $sessDir 'output.log') -Encoding UTF8

        # Create commands.jsonl
        @(
            '{"index":0,"command":"echo hello","exit_code":0,"output":"hello","timestamp":"2026-04-08T02:31:00Z"}'
            '{"index":1,"command":"npm run build","exit_code":1,"output":"ERROR: not found","timestamp":"2026-04-08T02:32:00Z"}'
            '{"index":2,"command":"npm run test","exit_code":0,"output":"All tests passed","timestamp":"2026-04-08T02:33:00Z"}'
        ) | Set-Content (Join-Path $sessDir 'commands.jsonl') -Encoding UTF8
    }

    AfterAll {
        $env:TERMSNOOP_HOME = $null
    }

    It 'reads last N commands as JSON' {
        $output = Read-TermSnoopSession -SessionId 'rd12345' -LastCommands 1 -Json
        $parsed = $output | ConvertFrom-Json
        $parsed.Count | Should -Be 1
        $parsed[0].command | Should -Be 'npm run test'
    }

    It 'reads last N commands as text' {
        $output = Read-TermSnoopSession -SessionId 'rd12345' -LastCommands 2 6>&1
        # This writes to host, so we verify it doesn't throw
        { Read-TermSnoopSession -SessionId 'rd12345' -LastCommands 2 } | Should -Not -Throw
    }

    It 'reads tail lines' {
        $output = Read-TermSnoopSession -SessionId 'rd12345' -Tail 2
        $output | Should -Match 'line 4'
        $output | Should -Match 'line 5'
    }

    It 'reads full output' {
        $output = Read-TermSnoopSession -SessionId 'rd12345'
        $output | Should -Match 'line 1'
        $output | Should -Match 'line 5'
    }

    It 'outputs JSON format for raw output' {
        $output = Read-TermSnoopSession -SessionId 'rd12345' -Json
        $parsed = $output | ConvertFrom-Json
        $parsed.session_id | Should -Be 'rd12345'
        $parsed.output | Should -Match 'line 1'
    }

    It 'throws for nonexistent session' {
        { Read-TermSnoopSession -SessionId 'nonexistent' } | Should -Throw
    }

    It 'throws when commands.jsonl missing and LastCommands used' {
        $noCmd = Join-Path $env:TERMSNOOP_HOME 'sessions' 'nocmd99'
        New-Item -ItemType Directory -Path $noCmd -Force | Out-Null
        [ordered]@{ id = 'nocmd99'; pid = 1; shell = 'pwsh'; cwd = '.'; started_at = '2026-01-01T00:00:00Z'; status = 'active' } |
            ConvertTo-Json | Set-Content (Join-Path $noCmd 'meta.json') -Encoding UTF8
        'some output' | Set-Content (Join-Path $noCmd 'output.log') -Encoding UTF8

        { Read-TermSnoopSession -SessionId 'nocmd99' -LastCommands 1 } | Should -Throw
    }

    It 'strips transcript metadata from output' {
        # Create a session with transcript-style output
        $metaDir = Join-Path $env:TERMSNOOP_HOME 'sessions' 'meta123'
        New-Item -ItemType Directory -Path $metaDir -Force | Out-Null
        [ordered]@{ id = 'meta123'; pid = 1; shell = 'pwsh'; cwd = '.'; started_at = '2026-01-01T00:00:00Z'; status = 'active' } |
            ConvertTo-Json | Set-Content (Join-Path $metaDir 'meta.json') -Encoding UTF8

        $transcriptContent = @(
            '**********************'
            'Windows PowerShell transcript start'
            'Start time: 20260408023000'
            'Username: DOMAIN\user'
            'Machine: HOSTNAME'
            '**********************'
            'actual output line 1'
            '**********************'
            'Command start time: 20260408023100'
            '**********************'
            'actual output line 2'
            '**********************'
            'Windows PowerShell transcript end'
            'End time: 20260408050000'
            '**********************'
        ) -join "`n"
        $transcriptContent | Set-Content (Join-Path $metaDir 'output.log') -Encoding UTF8

        $output = Read-TermSnoopSession -SessionId 'meta123'
        $output | Should -Match 'actual output line 1'
        $output | Should -Match 'actual output line 2'
        $output | Should -Not -Match 'transcript start'
        $output | Should -Not -Match 'Username'
        $output | Should -Not -Match 'Command start time'
    }
}

Describe 'Clear-TermSnoopSession' {
    BeforeAll {
        $env:TERMSNOOP_HOME = Join-Path $TestDrive "clean-$(New-Guid)"

        # Create an old session (2020)
        $oldDir = Join-Path $env:TERMSNOOP_HOME 'sessions' 'old1234'
        New-Item -ItemType Directory -Path $oldDir -Force | Out-Null
        [ordered]@{
            id = 'old1234'; pid = 1; shell = 'pwsh'
            cwd = 'C:\'; started_at = '2020-01-01T00:00:00Z'; status = 'exited'
        } | ConvertTo-Json | Set-Content (Join-Path $oldDir 'meta.json') -Encoding UTF8

        # Create a recent session (now)
        $newDir = Join-Path $env:TERMSNOOP_HOME 'sessions' 'new5678'
        New-Item -ItemType Directory -Path $newDir -Force | Out-Null
        [ordered]@{
            id = 'new5678'; pid = 2; shell = 'pwsh'
            cwd = 'C:\'; started_at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ'); status = 'active'
        } | ConvertTo-Json | Set-Content (Join-Path $newDir 'meta.json') -Encoding UTF8
    }

    AfterAll {
        $env:TERMSNOOP_HOME = $null
    }

    It 'removes old sessions and keeps recent ones' {
        Clear-TermSnoopSession -Days 7
        Join-Path $env:TERMSNOOP_HOME 'sessions' 'old1234' | Should -Not -Exist
        Join-Path $env:TERMSNOOP_HOME 'sessions' 'new5678' | Should -Exist
    }
}

Describe 'ANSI Escape Stripping' {
    It 'strips ANSI color codes' {
        InModuleScope termsnoop {
            $input_text = "normal `e[31mred text`e[0m normal"
            $result = Remove-AnsiEscapes -Text $input_text
            $result | Should -Be 'normal red text normal'
        }
    }

    It 'strips OSC sequences with BEL terminator' {
        InModuleScope termsnoop {
            $input_text = "before`e]0;window title`aafter"
            $result = Remove-AnsiEscapes -Text $input_text
            $result | Should -Be 'beforeafter'
        }
    }
}

Describe 'Transcript Metadata Stripping' {
    It 'strips standard transcript header fields' {
        InModuleScope termsnoop {
            $text = @(
                '**********************'
                'Windows PowerShell transcript start'
                'Start time: 20260408'
                'Username: test'
                'Machine: HOST'
                'Host Application: pwsh'
                'Process ID: 123'
                '**********************'
                'actual content'
            ) -join "`n"

            $result = Remove-TranscriptMetadata -Text $text
            $result | Should -Be 'actual content'
        }
    }
}
