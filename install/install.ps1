<#
.SYNOPSIS
    Install the greenpiste binary on Windows (and amont-agent, its old name, as a copy). Nothing else.

.DESCRIPTION
    irm https://raw.githubusercontent.com/greenharness/greenpiste/main/install/install.ps1 | iex

    The counterpart to install/install.sh, which is POSIX sh and therefore
    reaches Windows only through Git Bash. This one runs in the PowerShell
    that ships with Windows, so `curl | sh` stops being the only documented
    path on a platform the hooks genuinely support and CI genuinely tests.

    Like its sibling it deliberately turns NOTHING on. It downloads a verified
    binary, puts it where the shims already look, and says what to run next.
    An installer that enabled hooks — or set init.templateDir so every future
    clone got them — would contradict the project's central promise on first
    contact with the machine.

.PARAMETER Version
    A specific release, e.g. "v1.0.0". Defaults to the latest.

.PARAMETER BinDir
    Where to put the executable. Defaults to $HOME\.local\bin.
    `greenpiste install` writes this binary's ABSOLUTE path into
    settings.json, so the directory decides only whether you can type the
    command yourself — Claude Code finds it either way.
#>
[CmdletBinding()]
param(
    [string]$Version = $env:AMONT_AGENT_VERSION,
    [string]$BinDir  = $env:AMONT_AGENT_BIN_DIR
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'greenharness/greenpiste'
if (-not $BinDir)  { $BinDir  = Join-Path $HOME '.local\bin' }
if (-not $Version) { $Version = 'latest' }

function Write-Ok   ($m) { Write-Host "  [ok] $m"   -ForegroundColor Green }
function Write-Warn ($m) { Write-Host "  [!]  $m"   -ForegroundColor Yellow }
function Fail       ($m) { Write-Host "  [x]  $m"   -ForegroundColor Red; exit 1 }

# Windows PowerShell 5.1 still negotiates TLS 1.0 by default, which GitHub
# refuses outright — the download fails with a connection error that says
# nothing about the cause. PowerShell 7 already defaults to TLS 1.2+.
try {
    [Net.ServicePointManager]::SecurityProtocol =
        [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch {
    # Not fatal: PS7 has no mutable ServicePointManager and does not need one.
}

Write-Host ''
Write-Host '  greenpiste installer'
Write-Host ''

# Only x86_64 is published today. Windows on ARM runs x64 binaries through
# emulation, so this is a working install rather than a refusal — said out
# loud, because a silently emulated binary is a surprise worth naming.
$arch = $env:PROCESSOR_ARCHITECTURE
$target = 'x86_64-pc-windows-msvc'
if ($arch -eq 'ARM64') {
    Write-Warn 'ARM64 detected — installing the x64 build, which Windows runs under emulation.'
}

if ($Version -eq 'latest') {
    try {
        $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" `
            -Headers @{ 'User-Agent' = 'greenpiste-installer' }
        $Version = $release.tag_name
    } catch {
        Fail "could not determine the latest release (rate limited? pass -Version v1.2.3): $_"
    }
}
$v = $Version -replace '^v', ''

$name = "greenpiste-$v-$target"
$base = "https://github.com/$Repo/releases/download/v$v"

Write-Host "  version:  $v"
Write-Host "  platform: $target"
Write-Host "  into:     $BinDir"
Write-Host ''

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("greenpiste-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tmp -Force | Out-Null

try {
    $zip = Join-Path $tmp "$name.zip"
    $archiveDir = $env:GREENPISTE_ARCHIVE_DIR
    if ($archiveDir) {
        # A local archive (the release's standalone check): the same checks
        # as a download, read from a directory instead of the release.
        Write-Host "  from:     $archiveDir"
        try {
            Copy-Item -Path (Join-Path $archiveDir "$name.zip") -Destination $zip -ErrorAction Stop
        } catch {
            Fail "no $name.zip in $archiveDir"
        }
    } else {
        Write-Host '  downloading...'
        try {
            Invoke-WebRequest -Uri "$base/$name.zip" -OutFile $zip -UseBasicParsing
        } catch {
            Fail "download failed: $base/$name.zip"
        }
    }

    # Checksums are not optional here. This binary runs on every commit with
    # your credentials and reads every staged file; verifying what it is
    # before putting it in that position is the argument the project makes
    # about its own dependencies, applied to itself.
    $sums = Join-Path $tmp 'SHA256SUMS'
    try {
        if ($archiveDir) {
            Copy-Item -Path (Join-Path $archiveDir 'SHA256SUMS') -Destination $sums -ErrorAction Stop
        } else {
            Invoke-WebRequest -Uri "$base/SHA256SUMS" -OutFile $sums -UseBasicParsing
        }
    } catch {
        $sums = $null
        Write-Warn 'no SHA256SUMS published for this release - the download was NOT verified'
    }

    if ($sums) {
        $got  = (Get-FileHash -Path $zip -Algorithm SHA256).Hash.ToLower()
        $line = Select-String -Path $sums -Pattern ([regex]::Escape("$name.zip")) |
                Select-Object -First 1
        if (-not $line) { Fail "SHA256SUMS has no entry for $name.zip" }
        $want = ($line.Line -split '\s+')[0].ToLower()
        if ($got -ne $want) {
            Fail "checksum mismatch - refusing to install`n    expected $want`n    got      $got"
        }
        Write-Ok 'checksum verified'
    }

    Expand-Archive -Path $zip -DestinationPath $tmp -Force
    $src = Join-Path $tmp $name
    # Whether this is an upgrade decides what to say at the end: the
    # first-install steps are wrong advice the second time.
    $upgrading = Test-Path (Join-Path $BinDir 'greenpiste.exe')
    New-Item -ItemType Directory -Path $BinDir -Force | Out-Null

    $from = Join-Path $src 'greenpiste.exe'
    if (-not (Test-Path $from)) { throw "the archive holds no greenpiste.exe" }
    # The old name, amont-agent.exe, is placed the same way as a copy, so
    # hook entries baked to its path keep working.
    foreach ($exe in @('greenpiste.exe', 'amont-agent.exe')) {
        $to = Join-Path $BinDir $exe
        # Copy beside the destination, then move it into place: the move is
        # a rename in one directory, so the path holds the whole old file or
        # the whole new one, never a half-copied executable.
        $staged = "$to.new"
        Copy-Item -Path $from -Destination $staged -Force
        # Windows refuses to overwrite or delete a RUNNING executable (the
        # hook runs on most tool calls), but it allows renaming one: move
        # the current file aside first. A leftover from an earlier run is
        # removed when it is no longer running, else left for the next run.
        $aside = "$to.old"
        Remove-Item -Path $aside -Force -ErrorAction SilentlyContinue
        if (Test-Path $aside) {
            # The leftover is itself still running: set this one aside under
            # a name of its own.
            $aside = "$to.old-" + [guid]::NewGuid().ToString('N')
        }
        if (Test-Path $to) { Move-Item -Path $to -Destination $aside -Force }
        Move-Item -Path $staged -Destination $to -Force
        Remove-Item -Path $aside -Force -ErrorAction SilentlyContinue
        Write-Ok "installed $to"
    }
} finally {
    Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host ''

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$BinDir*") {
    Write-Warn "$BinDir is not on your PATH."
    Write-Host '       The shims find the binary there regardless; this is so YOU can run it:'
    Write-Host ''
    Write-Host "         [Environment]::SetEnvironmentVariable('Path', `"`$env:Path;$BinDir`", 'User')"
    Write-Host ''
}

if ($upgrading) {
    Write-Host '  Upgraded. The settings.json entry points at this path, so'
    Write-Host '  nothing needs rewiring. Confirm it still fires:'
    Write-Host ''
    Write-Host '    greenpiste doctor'
    Write-Host ''
} else {
    Write-Host '  Installed, and wired into nothing. The guard does not run until'
    Write-Host '  Claude Code is told about it:'
    Write-Host ''
    Write-Host '    greenpiste install          # print the settings block, write nothing'
    Write-Host '    greenpiste install --write  # merge it into the Claude settings.json'
    Write-Host '    greenpiste doctor           # installed, runnable, and actually firing?'
    Write-Host ''
    Write-Host '  Every rule but pipe-to-tail ships as observe, so nothing is refused'
    Write-Host '  until you have seen what it would have caught:'
    Write-Host ''
    Write-Host '    greenpiste status'
    Write-Host ''
}
