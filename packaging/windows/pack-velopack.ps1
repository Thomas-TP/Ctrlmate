param(
    [string]$Version = "",
    [string]$OutDir = "dist-velopack/win"
)

$ErrorActionPreference = "Stop"
$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $RepoRoot

if ([string]::IsNullOrWhiteSpace($Version)) {
    $cargo = Get-Content "Cargo.toml" -Raw
    if ($cargo -match 'version\s*=\s*"([^"]+)"') {
        $Version = $Matches[1]
    } else {
        throw "Could not determine workspace version from Cargo.toml"
    }
}

$DesktopExe = "target\release\ctrlmate-desktop.exe"
$DaemonExe  = "target\release\ctrlmated.exe"
$BridgeDir  = "crates\ctrlmate-lhm-bridge\bin\Release\net8.0-windows\win-x64"
$IconPath   = "apps\desktop\src-tauri\icons\icon.ico"

if (!(Test-Path $DesktopExe)) { throw "Missing $DesktopExe — run cargo build --release first." }
if (!(Test-Path $DaemonExe))  { throw "Missing $DaemonExe — run cargo build --release first." }

$StageDir = Join-Path $RepoRoot "dist-stage\win-x64"
if (Test-Path $StageDir) { Remove-Item $StageDir -Recurse -Force }
New-Item -ItemType Directory -Path $StageDir -Force | Out-Null
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

Copy-Item $DesktopExe $StageDir
Copy-Item $DaemonExe  $StageDir
Copy-Item "LICENSE"   $StageDir

if (Test-Path (Join-Path $BridgeDir "ctrlmate-lhm-bridge.exe")) {
    Write-Host "Bundling optional Deep Hardware bridge (ctrlmate-lhm-bridge.exe)..."
    Get-ChildItem $BridgeDir -File | Where-Object { $_.Extension -in ".exe", ".dll", ".json" } | Copy-Item -Destination $StageDir
}

Write-Host "Packing Ctrlmate v$Version with Velopack (vpk)..."
$vpkArgs = @(
    "pack",
    "--packId", "Ctrlmate",
    "--packVersion", $Version,
    "--packTitle", "Ctrlmate",
    "--packAuthors", "Ctrlmate Contributors",
    "--packDir", $StageDir,
    "--mainExe", "ctrlmate-desktop.exe",
    "--runtime", "win-x64",
    "--outputDir", $OutDir,
    "--channel", "win",
    "--shortcuts", "Desktop,StartMenuRoot"
)
if (Test-Path $IconPath) {
    $vpkArgs += @("--icon", $IconPath)
}

& vpk @vpkArgs
if ($LASTEXITCODE -ne 0) {
    throw "vpk pack failed with exit code $LASTEXITCODE"
}

Write-Host ""
Write-Host "Velopack Windows installer ready in $OutDir :"
Get-ChildItem $OutDir | Format-Table Name, Length -AutoSize
