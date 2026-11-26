<#
.SYNOPSIS
  Installs, upgrades or removes the Ctrlmate Windows service (ctrlmated, runs as SYSTEM).

.DESCRIPTION
  Needed for full hardware access (NZXT Kraken pump/fans, RGB controllers, GPU fan control,
  Windows Update, boot-time operation). The Velopack installer is per-user and never elevates;
  this script (or the "Install service" button in the app) performs the elevated step.

  It self-elevates through UAC, then runs the daemon's own installer, which:
    - copies ctrlmated.exe (and the optional deep-hardware bridge) to %ProgramFiles%\Ctrlmate (admin-write only),
    - registers the service with recovery actions,
    - creates the firewall rule (private + domain profiles, local subnet, TCP 9487 + UDP 5353),
    - applies the ACLs on %ProgramData%\Ctrlmate (SYSTEM + Administrators; ipc.token readable by Users).
  The session agent (ctrlmate-agent.exe) is NOT a service: it autostarts at login through
  HKCU\Software\Microsoft\Windows\CurrentVersion\Run, created by the app/agent.

.PARAMETER Action
  install (default), upgrade or uninstall.

.PARAMETER DaemonPath
  Path to ctrlmated.exe. Defaults to the one next to this script.
#>
[CmdletBinding()]
param(
    [ValidateSet("install", "upgrade", "uninstall")]
    [string]$Action = "install",
    [string]$DaemonPath = (Join-Path $PSScriptRoot "ctrlmated.exe")
)

$ErrorActionPreference = "Stop"

$flag = switch ($Action) {
    "install"   { "--install-service" }
    "upgrade"   { "--upgrade-service" }
    "uninstall" { "--uninstall-service" }
}

$principal = New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Host "Administrator rights are required; requesting elevation..."
    $argList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"", "-Action", $Action, "-DaemonPath", "`"$DaemonPath`"")
    $p = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList $argList -Verb RunAs -Wait -PassThru
    exit $p.ExitCode
}

if (-not (Test-Path $DaemonPath)) { throw "ctrlmated.exe not found at $DaemonPath" }

& $DaemonPath $flag
if ($LASTEXITCODE -ne 0) { throw "ctrlmated $flag failed with exit code $LASTEXITCODE" }

if ($Action -ne "uninstall") {
    $svc = Get-Service -Name "Ctrlmate" -ErrorAction SilentlyContinue
    if ($svc) { Write-Host "Service '$($svc.Name)': $($svc.Status)" }
    else { Write-Warning "Service not found after $Action. Check the daemon output above." }
} else {
    Write-Host "Service removed. Data in %ProgramData%\Ctrlmate was kept; delete it manually to reset pairing."
}
