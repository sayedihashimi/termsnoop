@{
    RootModule        = 'termsnoop.psm1'
    ModuleVersion     = '0.1.0'
    GUID              = 'd4c3b2a1-e5f6-4789-abcd-0123456789ab'
    Author            = 'Sayed Ibrahim Hashimi'
    Description       = 'Terminal capture for AI CLI integration. Uses PowerShell transcript to capture terminal output for AI tools like Copilot CLI and Claude CLI.'
    PowerShellVersion = '7.0'
    FunctionsToExport = @(
        'Start-TermSnoop'
        'Stop-TermSnoop'
        'Get-TermSnoopSession'
        'Read-TermSnoopSession'
        'Clear-TermSnoopSession'
    )
    CmdletsToExport   = @()
    VariablesToExport  = @()
    AliasesToExport    = @()
    PrivateData       = @{
        PSData = @{
            Tags       = @('terminal', 'capture', 'ai', 'copilot', 'transcript')
            LicenseUri = 'https://github.com/user/termsnoop/blob/main/LICENSE'
            ProjectUri = 'https://github.com/user/termsnoop'
        }
    }
}
