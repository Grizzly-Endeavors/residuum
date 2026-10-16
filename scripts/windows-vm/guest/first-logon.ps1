# Runs once, at the first autologon after the unattended install (see
# autounattend.xml.tmpl). Makes the VM reachable over SSH and keeps an
# interactive desktop session available for toast and UI checks.
# Written for Windows PowerShell 5.1, which is what a fresh install has.
param(
    [Parameter(Mandatory = $true)][string]$AnswerDrive
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

# Each step announces itself in C:\first-logon.log (this script's redirected
# output) and on COM1, which QEMU writes to guest-serial.log in the VM
# directory. The serial copy reaches the host before sshd is up, so create-vm.sh
# shows where a stalled first logon stopped.
function Write-Step([string]$Message) {
    $line = "first-logon: $Message"
    Write-Host $line
    try {
        $port = New-Object System.IO.Ports.SerialPort 'COM1', 115200
        $port.Open()
        $port.WriteLine($line)
        $port.Close()
    } catch {
        Write-Host "first-logon: could not write to COM1: $_"
    }
}
trap {
    Write-Step "FAILED: $($_.Exception.Message)"
    break
}

$harnessRoot = 'C:\residuum-harness'
New-Item -ItemType Directory -Force $harnessRoot | Out-Null

# Permanent autologon, so a desktop session exists after every boot. The
# unattend AutoLogon block only covers the first logon.
Write-Step 'autologon'
$password = (Get-Content -Raw (Join-Path $AnswerDrive 'password.txt')).Trim()
$winlogon = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
Set-ItemProperty $winlogon -Name AutoAdminLogon -Value '1'
Set-ItemProperty $winlogon -Name DefaultUserName -Value $env:USERNAME
Set-ItemProperty $winlogon -Name DefaultPassword -Value $password
Remove-ItemProperty $winlogon -Name AutoLogonCount -ErrorAction SilentlyContinue
net accounts /maxpwage:unlimited | Out-Null

# Client editions default to the Restricted execution policy, which blocks the
# harness scripts the host runs over SSH (provision.ps1, session-run.ps1).
# This script runs under -ExecutionPolicy Bypass, so Set-ExecutionPolicy saves
# the setting and then throws a terminating error about the process-scope
# override. That one error is expected; the saved value is checked instead.
Write-Step 'execution policy'
try {
    Set-ExecutionPolicy -Scope LocalMachine -ExecutionPolicy RemoteSigned -Force
} catch [System.Security.SecurityException] {
    if ($_.FullyQualifiedErrorId -notlike 'ExecutionPolicyOverride*') { throw }
}
if ((Get-ExecutionPolicy -Scope LocalMachine) -ne 'RemoteSigned') {
    throw 'failed to set the LocalMachine execution policy to RemoteSigned'
}

# Keep the session awake and unlocked so screenshots show the desktop.
Write-Step 'power and lock screen settings'
powercfg /change standby-timeout-ac 0
powercfg /change monitor-timeout-ac 0
powercfg /change hibernate-timeout-ac 0
$personalization = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Personalization'
New-Item -Force $personalization | Out-Null
Set-ItemProperty $personalization -Name NoLockScreen -Value 1 -Type DWord

# No surprise update reboots in the middle of a test run.
Write-Step 'Windows Update policy'
$au = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU'
New-Item -Force $au | Out-Null
Set-ItemProperty $au -Name NoAutoUpdate -Value 1 -Type DWord
Set-ItemProperty $au -Name NoAutoRebootWithLoggedOnUsers -Value 1 -Type DWord

# OpenSSH server from the upstream Win32-OpenSSH MSI. The Windows capability
# installs through Windows Update, which sat at 50% for over half an hour on a
# fresh Windows 11 evaluation image.
if (-not (Get-Service sshd -ErrorAction SilentlyContinue)) {
    Write-Step 'downloading the OpenSSH MSI'
    $release = Invoke-RestMethod 'https://api.github.com/repos/PowerShell/Win32-OpenSSH/releases/latest'
    $asset = $release.assets | Where-Object { $_.name -like 'OpenSSH-Win64-v*.msi' } | Select-Object -First 1
    $msi = Join-Path $env:TEMP $asset.name
    Invoke-WebRequest $asset.browser_download_url -OutFile $msi
    Write-Step 'installing the OpenSSH MSI'
    Start-Process msiexec.exe -ArgumentList '/i', "`"$msi`"", '/qn' -Wait
}
Write-Step 'starting sshd and opening its firewall port'
Set-Service sshd -StartupType Automatic
Start-Service sshd
# The MSI's own rule allows only the Private profile, and the QEMU user-mode
# network comes up as Public, so a rule for every profile is added.
if (Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue) {
    Set-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -Enabled True -Profile Any
} else {
    New-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -DisplayName 'OpenSSH Server (sshd)' `
        -Enabled True -Profile Any -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 | Out-Null
}

Write-Step 'sshd default shell and authorized key'
$openSshKey = 'HKLM:\SOFTWARE\OpenSSH'
New-Item -Force $openSshKey | Out-Null
Set-ItemProperty $openSshKey -Name DefaultShell `
    -Value 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'

# Members of Administrators authenticate against this file, and sshd ignores
# it unless only Administrators and SYSTEM can access it.
$adminKeys = 'C:\ProgramData\ssh\administrators_authorized_keys'
Copy-Item (Join-Path $AnswerDrive 'authorized_keys') $adminKeys -Force
icacls $adminKeys /inheritance:r /grant 'Administrators:F' /grant 'SYSTEM:F' | Out-Null
Restart-Service sshd

Set-Content (Join-Path $harnessRoot 'first-logon.done') (Get-Date -Format o)
Write-Step 'done'
