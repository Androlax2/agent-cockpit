# Parses and analyzes the shipped PowerShell launchers (#565, docs/packaging/windows.md). The
# release workflow's install-scoop job runs it under PowerShell 7 and under Windows PowerShell
# 5.1, the two shells Scoop's shims run in. A parse error, or any PSScriptAnalyzer error or
# warning, fails it. Never run it on a host that has not agreed to install a module.
$ErrorActionPreference = 'Stop'
$analyzerVersion = '1.24.0'
# The launchers print to the user's console on purpose: Write-Host keeps their messages out of
# any pipeline the caller builds around `torlek`.
$excludedRules = @('PSAvoidUsingWriteHost')
$launchers = @('torlek.ps1', 'torlek-wsl.ps1') | ForEach-Object { Join-Path $PSScriptRoot $_ }

$failed = $false
foreach ($launcher in $launchers) {
    $tokens = $null
    $parseErrors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($launcher, [ref] $tokens, [ref] $parseErrors)
    foreach ($parseError in $parseErrors) {
        [Console]::Error.WriteLine("$launcher($($parseError.Extent.StartLineNumber)): $($parseError.Message)")
        $failed = $true
    }
}

if (-not (Get-Module -ListAvailable -Name PSScriptAnalyzer | Where-Object { $_.Version -eq [version] $analyzerVersion })) {
    Install-Module -Name PSScriptAnalyzer -RequiredVersion $analyzerVersion -Scope CurrentUser -Force
}
Import-Module PSScriptAnalyzer -RequiredVersion $analyzerVersion
foreach ($launcher in $launchers) {
    $findings = @(Invoke-ScriptAnalyzer -Path $launcher -Severity Error, Warning -ExcludeRule $excludedRules)
    foreach ($finding in $findings) {
        [Console]::Error.WriteLine("$launcher($($finding.Line)): $($finding.Severity) $($finding.RuleName): $($finding.Message)")
        $failed = $true
    }
}

if ($failed) { exit 1 }
Write-Host "PowerShell $($PSVersionTable.PSVersion): parsed and analyzed $($launchers.Count) launchers with PSScriptAnalyzer $analyzerVersion, no findings."
