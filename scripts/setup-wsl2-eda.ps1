param(
    [string]$DistroName = "Ubuntu-24.04",
    [string]$InstallRoot = "$env:USERPROFILE\wsl\Ubuntu-24.04",
    [string]$RootfsPath = "$env:USERPROFILE\Downloads\ubuntu-24.04-wsl.rootfs.tar.gz"
)

$ErrorActionPreference = "Stop"

function Test-IsAdmin {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

if (-not (Test-IsAdmin)) {
    throw "Please run this script from an elevated PowerShell window."
}

$features = @(
    "Microsoft-Windows-Subsystem-Linux",
    "VirtualMachinePlatform",
    "Microsoft-Hyper-V-All"
)

foreach ($feature in $features) {
    Enable-WindowsOptionalFeature -Online -FeatureName $feature -All -NoRestart | Out-Null
}

wsl.exe --set-default-version 2

if (-not (Test-Path $RootfsPath)) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $RootfsPath) | Out-Null
    Invoke-WebRequest `
        -Uri "https://cloud-images.ubuntu.com/wsl/releases/24.04/current/ubuntu-noble-wsl-amd64-wsl.rootfs.tar.gz" `
        -OutFile $RootfsPath `
        -UseBasicParsing
}

$registered = (wsl.exe --list --quiet) -contains $DistroName
if (-not $registered) {
    New-Item -ItemType Directory -Force -Path $InstallRoot | Out-Null
    wsl.exe --import $DistroName $InstallRoot $RootfsPath --version 2
}

wsl.exe -d $DistroName -u root -- bash -lc "apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y verilator yosys build-essential git make python3"
wsl.exe -d $DistroName -u root -- bash -lc "verilator --version && yosys -V"

Write-Host ""
Write-Host "Done. If Windows asked for a reboot while enabling features, reboot and run this script again."
