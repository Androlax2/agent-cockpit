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

# The CI scenario runs only under PowerShell 7; a parse error there would otherwise surface
# after the job has set up WSL and Scoop.
if ($PSVersionTable.PSVersion.Major -ge 7) {
    $ciScript = Join-Path $PSScriptRoot 'ci-install.ps1'
    $tokens = $null
    $parseErrors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($ciScript, [ref] $tokens, [ref] $parseErrors)
    foreach ($parseError in $parseErrors) {
        [Console]::Error.WriteLine("$ciScript($($parseError.Extent.StartLineNumber)): $($parseError.Message)")
        $failed = $true
    }
}

if (-not (Get-Module -ListAvailable -Name PSScriptAnalyzer | Where-Object { $_.Version -eq [version] $analyzerVersion })) {
    # The PowerShell Gallery times out now and then (a merged run lost its job to one download).
    $maximumAttempts = 4
    for ($attempt = 1; ; $attempt++) {
        try {
            Install-Module -Name PSScriptAnalyzer -RequiredVersion $analyzerVersion -Scope CurrentUser -Force
            break
        } catch {
            if ($attempt -ge $maximumAttempts) { throw }
            Write-Host "PSScriptAnalyzer download failed (attempt $attempt of $maximumAttempts): $($_.Exception.Message)"
            Start-Sleep -Seconds (15 * $attempt)
        }
    }
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
