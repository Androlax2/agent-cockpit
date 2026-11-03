# torlek-wsl check|install [-Distribution NAME] [-Yes]
#
# Checks that a WSL 2 distribution can run the Linux x86_64 Torlek this Scoop package carries,
# and installs it there after the user agrees (docs/packaging/windows.md). It never installs or
# enables WSL, never adds, converts or removes a distribution, never elevates and never sets
# environment variables: each missing prerequisite is reported with the command to run.
param(
    [Parameter(Position = 0)]
    [ValidateSet('check', 'install')]
    [string] $Action = 'check',
    [string] $Distribution = $env:TORLEK_WSL_DISTRIBUTION,
    [switch] $Yes
)
$ErrorActionPreference = 'Stop'

function Stop-Check([string] $Message) {
    [Console]::Error.WriteLine("torlek-wsl: $Message")
    exit 1
}

function Get-RegisteredDistributions {
    # wsl.exe writes UTF-16 to a pipe unless WSL_UTF8=1; restore the caller's value afterwards,
    # because this script runs inside the user's PowerShell session.
    $previous = $env:WSL_UTF8
    try {
        $env:WSL_UTF8 = '1'
        $names = & wsl.exe --list --quiet
        if ($LASTEXITCODE -ne 0) { return @() }
        return @($names | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    } finally {
        $env:WSL_UTF8 = $previous
    }
}

if (-not (Get-Command wsl.exe -CommandType Application -ErrorAction SilentlyContinue)) {
    Stop-Check ('wsl.exe was not found: WSL is not installed. Torlek runs only inside WSL 2 and will not install it for you. ' +
        'If you choose to, run in an elevated terminal: wsl.exe --install --no-distribution ' +
        '(it enables Windows features and may need a restart), then add a distribution.')
}
$registered = Get-RegisteredDistributions
if ($registered.Count -eq 0) {
    Stop-Check ('No WSL distribution is registered. Adding one is your decision, for example: ' +
        'wsl.exe --install --distribution Ubuntu-24.04')
}
if (-not $Distribution) {
    Stop-Check ("Name the distribution to use: torlek-wsl $Action -Distribution <name>. Registered: " + ($registered -join ', '))
}
if ($registered -notcontains $Distribution) {
    Stop-Check ("WSL distribution '$Distribution' is not registered. Registered: " + ($registered -join ', '))
}

$archives = @(Get-ChildItem -LiteralPath $PSScriptRoot -Filter 'torlek-v*-linux-x86_64.archive')
if ($archives.Count -ne 1 -or $archives[0].Name -notmatch '^torlek-v(?<tag>.+)-linux-x86_64\.archive$') {
    Stop-Check "expected one torlek-v<tag>-linux-x86_64.archive in $PSScriptRoot, found $($archives.Count); reinstall the Scoop package"
}
$tag = $Matches['tag']

function Invoke-InDistribution([string] $Step) {
    & wsl.exe --distribution $Distribution --cd $PSScriptRoot --exec /bin/sh ./install-in-wsl.sh $Step $tag
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Invoke-InDistribution 'check'
if ($Action -eq 'check') { exit 0 }

if (-not $Yes) {
    $answer = Read-Host ("Install Torlek $tag into WSL distribution '$Distribution' at ~/.local/lib/torlek/v$tag " +
        'and point ~/.local/lib/torlek/current at it? Nothing else in the distribution changes. [y/N]')
    if ($answer -notmatch '^(y|yes)$') { Stop-Check 'Nothing was installed.' }
}
Invoke-InDistribution 'install'
if ($env:TORLEK_WSL_DISTRIBUTION -ne $Distribution) {
    Write-Host ('The torlek command reads the distribution from TORLEK_WSL_DISTRIBUTION. To set it for your account: ' +
        "[Environment]::SetEnvironmentVariable('TORLEK_WSL_DISTRIBUTION', '$Distribution', 'User')")
}
