# Builds a portable GamerTrials folder and zip for testing on another Windows
# machine: the client, the server, and everything they read at runtime, laid
# out as the repository root is, since every path they open is relative to it.
#
#   powershell -ExecutionPolicy Bypass -File tools\package.ps1 [-SkipBuild]
#
# ROMs are left out. The platform never distributes ROM bytes (CLAUDE.md):
# assets\ROMs\README.txt in the package lists each file the manifests expect,
# where it goes and the sha1 it must have, and the player supplies their own.

param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if (-not $SkipBuild) {
    cargo build --release -p app-client -p server-net
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
}

$commit = (git rev-parse --short HEAD).Trim()
if (git status --porcelain) { $commit = "$commit-dirty" }
$name = "GamerTrials-$commit"
$dist = Join-Path $root 'dist'
$out = Join-Path $dist $name
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null

function Copy-Tree($from, $to, $filter = '*', [switch]$Flat) {
    $src = Join-Path $root $from
    Get-ChildItem $src -Recurse:(-not $Flat) -File -Filter $filter | ForEach-Object {
        $rel = $_.FullName.Substring($src.Length).TrimStart('\')
        $dest = Join-Path (Join-Path $out $to) $rel
        New-Item -ItemType Directory -Force (Split-Path -Parent $dest) | Out-Null
        Copy-Item $_.FullName $dest
    }
}

Copy-Item target\release\app-client.exe (Join-Path $out 'GamerTrials.exe')
Copy-Item target\release\gamertrials-server.exe $out
# The server links the MSVC runtime dynamically and a fresh Windows may lack
# it; Microsoft allows this one file to sit beside the program that needs it.
Copy-Item (Join-Path $env:SystemRoot 'System32\vcruntime140.dll') $out
# The server and every client must hash the same manifests, so the package
# carries exactly the repository's.
Copy-Tree manifests manifests '*.toml'
Copy-Tree cores cores '*.dll'
# Not states\vault: captures kept for reference, named by no manifest.
Copy-Tree assets\states assets\states '*.state' -Flat
Copy-Tree assets\patches assets\patches '*.bps'

# Each [[rom]] a manifest names by path, with its sha1: what the player must
# put where. Patched ROMs carry a patch instead and are derived on first use.
$roms = foreach ($m in Get-ChildItem manifests -Filter *.toml) {
    $block = $null
    foreach ($line in Get-Content $m.FullName) {
        if ($line -match '^\s*\[\[rom\]\]') {
            if ($block -and $block.path) { $block }
            $block = @{ game = $m.BaseName }
            continue
        }
        if ($line -match '^\s*\[') { if ($block -and $block.path) { $block }; $block = $null; continue }
        if ($block -and $line -match '^\s*path\s*=\s*"(.+)"') { $block.path = $Matches[1] }
        if ($block -and $line -match '^\s*sha1\s*=\s*"([0-9a-f]+)"') { $block.sha1 = $Matches[1] }
    }
    if ($block -and $block.path) { $block }
}
New-Item -ItemType Directory -Force (Join-Path $out 'assets\ROMs') | Out-Null
$romText = @(
    'Put your own dumps here. GamerTrials ships no ROMs.'
    ''
    'Each file below goes at the path shown, relative to the GamerTrials folder,'
    'and must have exactly this sha1. The lobby shows any that are missing or'
    'differ; a round on a game one player cannot run is voided for everyone,'
    'so the host should enable only games every player has.'
    ''
)
foreach ($r in $roms) {
    $romText += "[$($r.game)]"
    $romText += "  $($r.path)"
    $romText += "  sha1 $($r.sha1)"
    $romText += ''
}
$romText += 'DOOM is the shareware 1.9 bundle zipped exactly as in the repository:'
$romText += 're-zipping it changes the hash (ADR 0018), so copy that zip, do not rebuild it.'
Set-Content -Encoding ascii (Join-Path $out 'assets\ROMs\README.txt') $romText

# A release client has no console, so its log goes to a file beside it.
Set-Content -Encoding ascii (Join-Path $out 'GamerTrials.cmd') @(
    '@echo off'
    'cd /d "%~dp0"'
    'start "GamerTrials" /min cmd /c ".\GamerTrials.exe %* > gamertrials.log 2>&1"'
)
Set-Content -Encoding ascii (Join-Path $out 'Start server.cmd') @(
    '@echo off'
    'cd /d "%~dp0"'
    '.\gamertrials-server.exe --listen 0.0.0.0:7878 --manifests manifests %*'
    'pause'
)

$protocol = (Select-String -Path crates\protocol\src\lib.rs -Pattern '^pub const VERSION: u32 = (\d+);').Matches[0].Groups[1].Value
Set-Content -Encoding ascii (Join-Path $out 'README.txt') @(
    "GamerTrials $commit (protocol v$protocol), built $(Get-Date -Format 'yyyy-MM-dd HH:mm')"
    ''
    '1. Copy your ROM dumps into assets\ROMs as assets\ROMs\README.txt lists.'
    '2. Run GamerTrials.cmd. It starts the client and writes gamertrials.log'
    '   here; send that file along with any bug report.'
    '   (GamerTrials.exe runs it too, without the log.)'
    '3. Join game takes the invite the host sends: host[:port]/CODE.'
    ''
    'To host a server on this machine, run "Start server.cmd" and forward TCP'
    '7878 and UDP 7878 to it: the lobby and the match ride the TCP port, and a'
    'head-to-head bout probes the UDP one so the two players can find each'
    'other and then play directly. Every player must run the same build: a'
    'client whose manifests hash differently from the server''s is refused,'
    'and says which hash it holds.'
    ''
    'A head-to-head bout is dealt only when the host ticks "Allow head-to-head'
    'challenges" in the lobby and at least two players are in; the Street'
    'Fighter II dump is the one it needs.'
    ''
    'Settings are kept in %APPDATA%\com.cwr.gamertrials\settings.toml.'
    'Needs Windows 10 or 11 with the WebView2 runtime (Windows 11 has it).'
)

$zip = "$out.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $out -DestinationPath $zip
$mb = [math]::Round((Get-Item $zip).Length / 1MB, 1)
Write-Host "Packaged $out"
Write-Host "      -> $zip ($mb MB)"
