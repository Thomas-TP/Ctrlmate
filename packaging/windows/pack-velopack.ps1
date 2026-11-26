<#
.SYNOPSIS
  Packs Ctrlmate for Windows with Velopack (per-user installer, no admin needed).

.DESCRIPTION
  Expects binaries already built:
    cargo build --release --locked -p ctrlmate-daemon -p ctrlmate-agent -p ctrlmate-desktop
    dotnet publish crates/ctrlmate-lhm-bridge -c Release -r win-x64 --self-contained false
  The version is read from [workspace.package].version (cargo metadata); -Version may only be
  used to assert it (the release workflow passes the tag version).

  Installed layout (%LocalAppData%\Ctrlmate\current): ctrlmate-desktop.exe (main), ctrlmated.exe,
  ctrlmate-agent.exe, ctrlmate-lhm-bridge.* (optional deep hardware bridge), install-service.ps1.
  The elevated service install/upgrade (ctrlmated --install-service / --upgrade-service) is done
  from the app or with install-service.ps1; the installer itself never elevates.
  The agent autostart (HKCU\Software\Microsoft\Windows\CurrentVersion\Run) is created by the app/agent.

  Signing (optional, skipped with a visible warning when not configured):
    -SignParams / $env:CTRLMATE_SIGN_PARAMS             signtool arguments, e.g. '/f cert.pfx /p ... /tr http://timestamp.digicert.com /td sha256 /fd sha256'
    -AzureTrustedSignFile / $env:CTRLMATE_AZURE_SIGN_FILE   Azure Trusted Signing metadata.json (needs az login / OIDC)

.PARAMETER DownloadPrevious
  Download the previous GitHub release into OutDir first so vpk generates delta packages.
#>
[CmdletBinding()]
param(
    [string]$Version = "",
    [string]$OutDir = "dist-release/win",
    [string]$Channel = "win",
    [string]$RepoUrl = "https://github.com/Thomas-TP/Ctrlmate",
    [switch]$DownloadPrevious,
    [switch]$Prerelease,
    [string]$GithubToken = $env:GITHUB_TOKEN,
    [string]$SignParams = $env:CTRLMATE_SIGN_PARAMS,
    [string]$AzureTrustedSignFile = $env:CTRLMATE_AZURE_SIGN_FILE,
    [string]$Framework = "webview2,net10-x64-desktop"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
Set-Location $RepoRoot

function Invoke-Native {
    param([string]$Exe, [string[]]$Arguments)
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Exe $($Arguments[0]) failed with exit code $LASTEXITCODE" }
}

# --- version: single source of truth is the workspace -----------------------------------------
$meta = (& cargo metadata --no-deps --format-version 1 --locked) | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
$daemon = $meta.packages | Where-Object { $_.name -eq "ctrlmate-daemon" }
if (-not $daemon) { throw "ctrlmate-daemon not found in workspace" }
$WorkspaceVersion = $daemon.version
if ($Version -and $Version -ne $WorkspaceVersion) {
    throw "Requested version '$Version' differs from workspace version '$WorkspaceVersion'"
}
$Version = $WorkspaceVersion
Write-Host "Ctrlmate version: $Version"

# --- inputs -----------------------------------------------------------------------------------
$Release = Join-Path $RepoRoot "target\release"
$Required = @("ctrlmate-desktop.exe", "ctrlmated.exe", "ctrlmate-agent.exe")
foreach ($f in $Required) {
    if (-not (Test-Path (Join-Path $Release $f))) { throw "Missing $Release\$f. Build with cargo build --release --locked first." }
}
$BridgeCandidates = @(
    "crates\ctrlmate-lhm-bridge\bin\Release\net10.0-windows\win-x64\publish",
    "crates\ctrlmate-lhm-bridge\bin\Release\net10.0-windows\win-x64"
)
$BridgeDir = $BridgeCandidates | Where-Object { Test-Path (Join-Path $_ "ctrlmate-lhm-bridge.exe") } | Select-Object -First 1
$Icon = "apps\desktop\src-tauri\icons\icon.ico"

# --- stage ------------------------------------------------------------------------------------
$Stage = Join-Path $RepoRoot "dist-stage\win-x64"
if (Test-Path $Stage) { Remove-Item $Stage -Recurse -Force }
New-Item -ItemType Directory -Path $Stage -Force | Out-Null
foreach ($f in $Required) { Copy-Item (Join-Path $Release $f) $Stage }
Copy-Item "LICENSE" $Stage
Copy-Item "packaging\windows\install-service.ps1" $Stage
if ($BridgeDir) {
    Write-Host "Bundling deep-hardware bridge from $BridgeDir"
    Get-ChildItem $BridgeDir -File | Where-Object { $_.Extension -in ".exe", ".dll", ".json" -and $_.Name -notlike "*.pdb" } |
        Copy-Item -Destination $Stage
} else {
    Write-Warning "ctrlmate-lhm-bridge not built: the package will not contain the optional deep-hardware bridge."
}

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null
$OutDir = (Resolve-Path $OutDir).Path

# --- previous release for delta packages --------------------------------------------------------
if ($DownloadPrevious) {
    Write-Host "Downloading previous release from $RepoUrl (channel $Channel) for delta generation..."
    $dl = @("download", "github", "--repoUrl", $RepoUrl, "--channel", $Channel, "--outputDir", $OutDir)
    if ($GithubToken) { $dl += @("--token", $GithubToken) }
    if ($Prerelease) { $dl += "--pre" }
    & vpk @dl
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "No previous release could be downloaded (first release?). A full package will be produced without delta."
    }
}

# --- pack -------------------------------------------------------------------------------------
$vpkArgs = @(
    "pack",
    "--packId", "Ctrlmate",
    "--packVersion", $Version,
    "--packTitle", "Ctrlmate",
    "--packAuthors", "Ctrlmate Contributors",
    "--packDir", $Stage,
    "--mainExe", "ctrlmate-desktop.exe",
    "--runtime", "win-x64",
    "--outputDir", $OutDir,
    "--channel", $Channel,
    "--shortcuts", "Desktop,StartMenuRoot"
)
if ($Framework) { $vpkArgs += @("--framework", $Framework) }
if (Test-Path $Icon) { $vpkArgs += @("--icon", $Icon) }

if ($AzureTrustedSignFile) {
    Write-Host "Authenticode: Azure Trusted Signing ($AzureTrustedSignFile)"
    $vpkArgs += @("--azureTrustedSignFile", $AzureTrustedSignFile)
} elseif ($SignParams) {
    Write-Host "Authenticode: signtool"
    $vpkArgs += @("--signParams", $SignParams)
} else {
    $msg = "Authenticode signing is NOT configured: the installer and binaries will be UNSIGNED (Windows SmartScreen will warn). Set CTRLMATE_SIGN_PARAMS or CTRLMATE_AZURE_SIGN_FILE."
    Write-Warning $msg
    if ($env:GITHUB_ACTIONS) { Write-Host "::warning title=Unsigned Windows build::$msg" }
}

Write-Host "Packing with Velopack..."
Invoke-Native "vpk" $vpkArgs

# --- validate outputs -------------------------------------------------------------------------
$expected = @(
    "Ctrlmate-$Channel-Setup.exe",
    # Velopack omits the channel from the full package name on its default channel (win): `Ctrlmate-<version>-full.nupkg`.
    "Ctrlmate-$Version-full.nupkg",
    "releases.$Channel.json"
)
$missing = $expected | Where-Object { -not (Test-Path (Join-Path $OutDir $_)) }
if ($missing) {
    Write-Host "Files in ${OutDir}:"; Get-ChildItem $OutDir | ForEach-Object { Write-Host "  $($_.Name)" }
    throw "Velopack output incomplete, missing: $($missing -join ', ')"
}
$feed = Get-Content (Join-Path $OutDir "releases.$Channel.json") -Raw | ConvertFrom-Json
$latest = $feed.Assets | Where-Object { $_.Type -eq "Full" } | Sort-Object { [version]($_.Version -replace '-.*$', '') } | Select-Object -Last 1
if (-not $latest -or $latest.Version -ne $Version) { throw "Feed does not list full package for $Version" }
if (-not ($feed.Assets | Where-Object { $_.Type -eq "Delta" })) {
    Write-Warning "No delta package generated (expected for the first release or when -DownloadPrevious was not used)."
}
Write-Host "Velopack Windows packages for $Version ready in ${OutDir}:"
Get-ChildItem $OutDir | Format-Table Name, Length -AutoSize
