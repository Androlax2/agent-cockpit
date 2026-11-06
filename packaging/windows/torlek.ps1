# Windows launcher for Torlek: runs the Linux `torlek` inside the declared WSL 2 distribution.
# There is no native Windows Torlek; this script only forwards its arguments into WSL.
# Shipped by the Scoop manifest rendered from packaging/windows/scoop/torlek.json.in
# (docs/packaging/windows.md); torlek-wsl.ps1 checks and installs the Linux side.
$ErrorActionPreference = 'Stop'

function Exit-Launcher([string] $Message) {
    [Console]::Error.WriteLine("torlek: $Message")
    exit 1
}

$distribution = $env:TORLEK_WSL_DISTRIBUTION
if (-not $distribution) {
    Exit-Launcher ('TORLEK_WSL_DISTRIBUTION is not set. Name the WSL 2 distribution Torlek is installed in, ' +
        'for example: [Environment]::SetEnvironmentVariable(''TORLEK_WSL_DISTRIBUTION'', ''Ubuntu-24.04'', ''User''), ' +
        'then open a new terminal. Run torlek-wsl check first.')
}
if (-not (Get-Command wsl.exe -CommandType Application -ErrorAction SilentlyContinue)) {
    Exit-Launcher 'wsl.exe was not found: WSL is not installed. Torlek runs only inside WSL 2; run torlek-wsl check for the next step.'
}

# One probe both finds the Linux home and confirms the install, so a missing distribution or a
# missing install stops here with a next step instead of a bare exec error from wsl.exe.
$probe = 'test -x $HOME/.local/lib/torlek/current/bin/torlek && printf %s $HOME'
$linuxHome = & wsl.exe --distribution $distribution --exec /bin/sh -c $probe
if ($LASTEXITCODE -ne 0 -or -not $linuxHome) {
    Exit-Launcher ("Torlek is not installed in WSL distribution '$distribution', or the distribution did not start " +
        '(wsl.exe prints the reason above when it is the distribution). Run: torlek-wsl check')
}
& wsl.exe --distribution $distribution --exec "$linuxHome/.local/lib/torlek/current/bin/torlek" @args
exit $LASTEXITCODE
