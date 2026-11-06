# The release workflow's clean Windows + WSL 2 Scoop install (#565, docs/packaging/windows.md):
# on a hosted Windows runner with an Ubuntu 24.04 WSL 2 distribution, render the manifest for
# the candidate, install it with a pinned Scoop, run the refusals, install Torlek into WSL
# after the consent prompt, verify the installed bundle, then uninstall the Scoop package and
# check that the distribution and its files are untouched. Never run it on a host: it installs
# software and changes the user's PATH-visible shims.
#
# Usage: ci-install.ps1 -Dist DIST -Version VERSION -Revision REVISION -ScoopCore DIR
#   DIST holds SHA256SUMS and torlek-vVERSION-linux-x86_64.tar.gz; VERSION is a dry run's
#   X.Y.Z-dev.N; REVISION is the commit the launchers are hashed at; SCOOP_CORE is a checkout of
#   Scoop at a pinned commit, run in place so that the test does not follow Scoop's master.
param(
    [Parameter(Mandatory)] [string] $Dist,
    [Parameter(Mandatory)] [string] $Version,
    [Parameter(Mandatory)] [string] $Revision,
    [Parameter(Mandatory)] [string] $ScoopCore,
    [string] $Distribution = 'Ubuntu-24.04',
    [int] $Port = 8765
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($env:GITHUB_ACTIONS -ne 'true') {
    throw 'ci-install.ps1: refusing to run outside GitHub Actions'
}

$repositoryRoot = (Get-Location).Path
$dist = (Resolve-Path -LiteralPath $Dist).Path
$scoopScript = Join-Path (Resolve-Path -LiteralPath $ScoopCore).Path 'bin/scoop.ps1'
$bundleName = "torlek-v$Version-linux-x86_64"
$workRoot = Join-Path $env:RUNNER_TEMP 'torlek-scoop'
$bucket = Join-Path $workRoot 'bucket'
$env:SCOOP = Join-Path $workRoot 'scoop'
$shims = Join-Path $env:SCOOP 'shims'
$appDirectory = Join-Path $env:SCOOP 'apps/torlek/current'
$pwshPath = (Get-Command pwsh -CommandType Application).Source
$windowsPowerShellPath = Join-Path $env:SystemRoot 'System32/WindowsPowerShell/v1.0/powershell.exe'
$linuxRoot = '.local/lib/torlek'
# install.sh's layout (#1047): the bundle beside earlier ones, and ~/.local/bin/torlek linked into it.
$linuxBundle = "$linuxRoot/$bundleName"
$linuxLink = '.local/bin/torlek'
$shellTimeoutSeconds = 180

function Invoke-InShell {
    <# Runs one script file in a child shell and returns its exit code and combined output. #>
    param(
        [Parameter(Mandatory)] [string] $Shell,
        [Parameter(Mandatory)] [string] $Script,
        [string[]] $Arguments = @(),
        [string] $Stdin,
        [hashtable] $Environment = @{}
    )
    $saved = @{}
    foreach ($name in $Environment.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name)
        [Environment]::SetEnvironmentVariable($name, $Environment[$name])
    }
    try {
        $startInfo = [System.Diagnostics.ProcessStartInfo]::new($Shell)
        foreach ($argument in (@('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $Script) + $Arguments)) {
            $startInfo.ArgumentList.Add($argument)
        }
        $startInfo.UseShellExecute = $false
        $startInfo.RedirectStandardInput = $true
        $startInfo.RedirectStandardOutput = $true
        $startInfo.RedirectStandardError = $true
        $process = [System.Diagnostics.Process]::Start($startInfo)
        $outputTask = $process.StandardOutput.ReadToEndAsync()
        $errorTask = $process.StandardError.ReadToEndAsync()
        # Stdin is always closed, so nothing a launcher or wsl.exe runs can wait on a terminal.
        if ($PSBoundParameters.ContainsKey('Stdin')) { $process.StandardInput.WriteLine($Stdin) }
        $process.StandardInput.Close()
        if (-not $process.WaitForExit($shellTimeoutSeconds * 1000)) {
            $process.Kill($true)
            $process.WaitForExit()
            # A descendant that outlived the kill could still hold a pipe open: do not wait on it.
            [void] [System.Threading.Tasks.Task]::WaitAll([System.Threading.Tasks.Task[]] @($outputTask, $errorTask), 5000)
            $partialOutput = (@($outputTask, $errorTask) | ForEach-Object { if ($_.IsCompleted) { $_.Result } }) -join ''
            throw "$Script hung for more than ${shellTimeoutSeconds}s. Output so far:`n$partialOutput"
        }
        $process.WaitForExit()
        return [pscustomobject]@{ ExitCode = $process.ExitCode; Output = $outputTask.Result + $errorTask.Result }
    } finally {
        foreach ($name in $saved.Keys) {
            [Environment]::SetEnvironmentVariable($name, $saved[$name])
        }
    }
}

function Invoke-Wsl {
    <# Runs one program in the distribution, with its Linux home as the starting directory. #>
    param([Parameter(Mandatory)] [string[]] $Command, [string] $WorkingDirectory, [string] $User)
    $previousUtf8 = $env:WSL_UTF8
    $previousPreference = $ErrorActionPreference
    $env:WSL_UTF8 = '1'
    $ErrorActionPreference = 'Continue'
    try {
        $arguments = @('--distribution', $Distribution)
        if ($WorkingDirectory) { $arguments += @('--cd', $WorkingDirectory) }
        if ($User) { $arguments += @('--user', $User) }
        $output = & wsl.exe @arguments --exec @Command 2>&1 | Out-String
        return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
    } finally {
        $ErrorActionPreference = $previousPreference
        $env:WSL_UTF8 = $previousUtf8
    }
}

function Assert-Result {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [Parameter(Mandatory)] $Result,
        [Parameter(Mandatory)] [ValidateSet('succeeds', 'is refused')] [string] $Expectation,
        [string] $Pattern
    )
    $succeeded = $Result.ExitCode -eq 0
    $problem = $null
    if (($Expectation -eq 'succeeds') -ne $succeeded) {
        $problem = "exit code $($Result.ExitCode), but it should have $(if ($Expectation -eq 'succeeds') { 'succeeded' } else { 'been refused' })"
    } elseif ($Pattern -and $Result.Output -notmatch $Pattern) {
        $problem = "output does not match /$Pattern/"
    }
    if ($problem) {
        Write-Host $Result.Output
        throw "${Name}: $problem"
    }
    Write-Host "ok: $Name $Expectation"
}

function Assert-True {
    param([Parameter(Mandatory)] [string] $Name, [Parameter(Mandatory)] [bool] $Condition)
    if (-not $Condition) { throw "${Name}: failed" }
    Write-Host "ok: $Name"
}

function Test-InstalledInWsl {
    # A failing wsl.exe must not read as "not installed": the negative checks would pass unseen.
    $probe = Invoke-Wsl -Command @('/bin/sh', '-c', "if test -d `$HOME/$linuxBundle; then echo installed; else echo absent; fi")
    if ($probe.ExitCode -ne 0 -or $probe.Output.Trim() -notin @('installed', 'absent')) {
        Write-Host $probe.Output
        throw "could not look for the Linux install (exit code $($probe.ExitCode))"
    }
    return $probe.Output.Trim() -eq 'installed'
}

function Set-DefaultUser {
    <# The launchers run wsl.exe without --user, so the distribution's default user must be ordinary. #>
    param([Parameter(Mandatory)] [string] $User)
    $configure = Invoke-Wsl -User root -Command @('/bin/sh', '-c', "printf '[user]\ndefault=$User\n' >> /etc/wsl.conf")
    if ($configure.ExitCode -ne 0) { throw "could not set the default WSL user: $($configure.Output)" }
    & wsl.exe --terminate $Distribution
    if ($LASTEXITCODE -ne 0) { throw 'wsl.exe --terminate failed' }
}

function Get-RegisteredDistribution {
    $previousUtf8 = $env:WSL_UTF8
    $env:WSL_UTF8 = '1'
    try {
        return @(& wsl.exe --list --quiet | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    } finally {
        $env:WSL_UTF8 = $previousUtf8
    }
}

function Invoke-Scoop {
    param([Parameter(Mandatory)] [string[]] $Arguments)
    return Invoke-InShell -Shell $pwshPath -Script $scoopScript -Arguments $Arguments
}

function Invoke-Launcher {
    param(
        [Parameter(Mandatory)] [string] $Name,
        [string[]] $Arguments = @(),
        [string] $Shell = $pwshPath,
        [string] $Stdin,
        [hashtable] $Environment = @{ TORLEK_WSL_DISTRIBUTION = $null },
        # Scoop's .ps1 shim turns piped input into pipeline input for the script, which the
        # launcher does not accept; a person answers the prompt at the keyboard, so the
        # scripted answer goes to the installed script itself.
        [switch] $Direct
    )
    $parameters = @{
        Shell = $Shell
        Script = (Join-Path ($Direct ? $appDirectory : $shims) "$Name.ps1")
        Arguments = $Arguments
        Environment = $Environment
    }
    if ($PSBoundParameters.ContainsKey('Stdin')) { $parameters.Stdin = $Stdin }
    return Invoke-InShell @parameters
}

Write-Host '::group::Preconditions'
Assert-True 'the checkout is the candidate revision' ((& git rev-parse HEAD) -eq $Revision)
Assert-True 'the distribution is registered' ((Get-RegisteredDistribution) -contains $Distribution)
Assert-Result 'the distribution runs a WSL 2 kernel' (Invoke-Wsl -Command @('uname', '-r')) succeeds 'microsoft-standard-WSL2'
if ((Invoke-Wsl -Command @('id', '-u')).Output.Trim() -eq '0') {
    # setup-wsl's wsl-shell-user applies to its wsl-bash wrapper; wsl.exe without --user may start root.
    Set-DefaultUser -User cockpit
}
Assert-Result 'the distribution user is not root' (Invoke-Wsl -Command @('/bin/sh', '-c', 'test $(id -u) -ne 0')) succeeds
Assert-Result 'the distribution starts without Torlek' (Invoke-Wsl -Command @('/bin/sh', '-c', "test ! -e `$HOME/$linuxRoot")) succeeds
# The user's own state, which uninstalling must leave alone.
Assert-Result 'user state is seeded' (Invoke-Wsl -Command @('/bin/sh', '-c', 'mkdir -p $HOME/torlek-user-state && echo kept > $HOME/torlek-user-state/marker')) succeeds
Write-Host '::endgroup::'

Write-Host '::group::Render the manifest and serve its files from loopback'
# The runner also lists the Microsoft Store stub; PATH order puts the real interpreter first.
$python = @(Get-Command python -CommandType Application -ErrorAction Stop)[0]
& $python.Source packaging/windows/render.py rehearse --tag $Version `
    --sums (Join-Path $dist 'SHA256SUMS') --archive (Join-Path $dist "$bundleName.tar.gz") `
    --revision $Revision --url "http://127.0.0.1:$Port" --out $bucket
if ($LASTEXITCODE -ne 0) { throw 'render.py rehearse failed' }
$server = Start-Process -FilePath $python.Source -PassThru -NoNewWindow `
    -ArgumentList @('-m', 'http.server', "$Port", '--bind', '127.0.0.1', '--directory', $bucket)
Write-Host '::endgroup::'

try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 30 -and -not $ready; $attempt++) {
        try {
            $ready = (Invoke-WebRequest -UseBasicParsing "http://127.0.0.1:$Port/torlek.json").StatusCode -eq 200
        } catch {
            Start-Sleep -Seconds 1
        }
    }
    Assert-True 'the loopback server answers' $ready

    Write-Host '::group::scoop install'
    $install = Invoke-Scoop -Arguments @('install', (Join-Path $bucket 'torlek.json'))
    Write-Host $install.Output
    Assert-Result 'scoop install of the rendered manifest' $install succeeds
    Write-Host '::endgroup::'

    $installedFiles = @(Get-ChildItem -LiteralPath $appDirectory -File | ForEach-Object Name | Where-Object { $_ -notin @('scoop-manifest.json', 'scoop-install.json') } | Sort-Object)
    $expectedFiles = @('SHA256SUMS', 'install-in-wsl.sh', 'install.sh', "$bundleName.archive", 'torlek-wsl.ps1', 'torlek.ps1') | Sort-Object
    Write-Host "installed files: $($installedFiles -join ', ')"
    Write-Host "expected files:  $($expectedFiles -join ', ')"
    Assert-True 'Scoop downloaded exactly the six bound files' (($installedFiles -join ',') -eq ($expectedFiles -join ','))
    Assert-True 'Scoop shimmed torlek and torlek-wsl' ((Test-Path (Join-Path $shims 'torlek.ps1')) -and (Test-Path (Join-Path $shims 'torlek-wsl.ps1')))
    Assert-True 'scoop install changed nothing in WSL' (-not (Test-InstalledInWsl))

    foreach ($shell in @($pwshPath, $windowsPowerShellPath)) {
        $shellName = Split-Path -Leaf $shell
        $ready = Invoke-Launcher -Name 'torlek-wsl' -Arguments @('check', '-Distribution', $Distribution) -Shell $shell
        Assert-Result "torlek-wsl check under $shellName" $ready succeeds "verified; installing would unpack Torlek v$([regex]::Escape($Version))"
        Assert-Result "torlek-wsl check, unknown distribution, under $shellName" (Invoke-Launcher -Name 'torlek-wsl' -Arguments @('check', '-Distribution', 'NoSuchDistribution') -Shell $shell) 'is refused' 'is not registered'
        Assert-Result "torlek-wsl check, no distribution named, under $shellName" (Invoke-Launcher -Name 'torlek-wsl' -Arguments @('check') -Shell $shell) 'is refused' 'Name the distribution'
        Assert-Result "torlek, no distribution named, under $shellName" (Invoke-Launcher -Name 'torlek' -Arguments @('version') -Shell $shell) 'is refused' 'TORLEK_WSL_DISTRIBUTION is not set'
    }
    # The shim cmd.exe users run: it starts PowerShell itself and forwards the arguments.
    $cmdShim = Join-Path $shims 'torlek-wsl.cmd'
    Assert-True 'Scoop wrote a cmd shim' (Test-Path -LiteralPath $cmdShim)
    $viaCmd = & $cmdShim check -Distribution $Distribution 2>&1 | Out-String
    $viaCmdExitCode = $LASTEXITCODE
    Write-Host $viaCmd
    Assert-True 'torlek-wsl check through the cmd shim' (($viaCmdExitCode -eq 0) -and ($viaCmd -match 'verified; installing would unpack'))
    Assert-Result 'torlek before the Linux install' (Invoke-Launcher -Name 'torlek' -Arguments @('version') -Environment @{ TORLEK_WSL_DISTRIBUTION = $Distribution }) 'is refused' 'is not installed in WSL distribution'
    # PATH with pwsh but without System32: the launcher must report the missing wsl.exe.
    Assert-Result 'torlek-wsl check without wsl.exe' (Invoke-Launcher -Name 'torlek-wsl' -Arguments @('check', '-Distribution', $Distribution) -Environment @{ PATH = (Split-Path -Parent $pwshPath); TORLEK_WSL_DISTRIBUTION = $null }) 'is refused' 'wsl\.exe was not found'

    Assert-Result 'declining the consent prompt' (Invoke-Launcher -Name 'torlek-wsl' -Arguments @('install', '-Distribution', $Distribution) -Stdin 'n' -Direct) 'is refused' 'Nothing was installed'
    Assert-True 'declining installed nothing' (-not (Test-InstalledInWsl))

    $archivePath = Join-Path $appDirectory "$bundleName.archive"
    $originalArchive = [IO.File]::ReadAllBytes($archivePath)
    try {
        [IO.File]::WriteAllBytes($archivePath, $originalArchive + [byte[]] @(0))
        Assert-Result 'torlek-wsl check on a tampered archive' (Invoke-Launcher -Name 'torlek-wsl' -Arguments @('check', '-Distribution', $Distribution)) 'is refused' 'integrity check failed'
        Assert-Result 'torlek-wsl install on a tampered archive' (Invoke-Launcher -Name 'torlek-wsl' -Arguments @('install', '-Distribution', $Distribution, '-Yes')) 'is refused' 'integrity check failed'
        Assert-True 'the tampered archive installed nothing' (-not (Test-InstalledInWsl))
    } finally {
        [IO.File]::WriteAllBytes($archivePath, $originalArchive)
    }

    $installed = Invoke-Launcher -Name 'torlek-wsl' -Arguments @('install', '-Distribution', $Distribution, '-Yes')
    Write-Host $installed.Output
    Assert-Result 'torlek-wsl install' $installed succeeds "installed Torlek v$([regex]::Escape($Version))"
    Assert-Result 'a second install changes nothing' (Invoke-Launcher -Name 'torlek-wsl' -Arguments @('install', '-Distribution', $Distribution, '-Yes')) succeeds 'is already installed at .*; nothing changed'

    $verify = Invoke-Wsl -WorkingDirectory $repositoryRoot -Command @('/bin/sh', '-c', "python3 tools/release.py verify-install `$HOME/$linuxBundle --revision $Revision")
    Write-Host $verify.Output
    Assert-Result 'the installed bundle verifies against the candidate revision' $verify succeeds

    $launcherEnvironment = @{ TORLEK_WSL_DISTRIBUTION = $Distribution }
    $versionOutput = Invoke-Launcher -Name 'torlek' -Arguments @('version', '--all') -Environment $launcherEnvironment
    Write-Host $versionOutput.Output
    Assert-Result 'torlek version --all through the Windows launcher' $versionOutput succeeds 'build release'
    # No provider, tmux 3.6 or systemd user manager is set up here, so doctor may exit 1 for a
    # missing prerequisite; its own lines must still show the packaged release build.
    $doctor = Invoke-Launcher -Name 'torlek' -Arguments @('doctor') -Environment $launcherEnvironment
    Write-Host $doctor.Output
    Assert-True 'torlek doctor ran through the Windows launcher' ($doctor.ExitCode -le 1)
    Assert-True 'torlek doctor names the packaged release build' ($doctor.Output -match 'build release')

    Write-Host '::group::scoop uninstall'
    $uninstall = Invoke-Scoop -Arguments @('uninstall', 'torlek')
    Write-Host $uninstall.Output
    Assert-Result 'scoop uninstall' $uninstall succeeds
    Write-Host '::endgroup::'
    Assert-True 'uninstall removed the app directory and shims' (-not (Test-Path (Join-Path $env:SCOOP 'apps/torlek')) -and -not (Test-Path (Join-Path $shims 'torlek.ps1')))
    Assert-True 'uninstall kept the distribution' ((Get-RegisteredDistribution) -contains $Distribution)
    Assert-True 'uninstall kept the Linux install' (Test-InstalledInWsl)
    Assert-Result 'uninstall kept the torlek link' (Invoke-Wsl -Command @('/bin/sh', '-c', "test `$(readlink `$HOME/$linuxLink) = `$HOME/$linuxBundle/bin/torlek")) succeeds
    Assert-Result 'uninstall kept the user state' (Invoke-Wsl -Command @('/bin/sh', '-c', 'test $(cat $HOME/torlek-user-state/marker) = kept')) succeeds
} finally {
    Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
}
Write-Host "torlek $Version : rendered, installed with Scoop, installed into WSL, verified and uninstalled on Windows"
