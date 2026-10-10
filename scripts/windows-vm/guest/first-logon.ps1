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

$harnessRoot = 'C:\residuum-harness'
New-Item -ItemType Directory -Force $harnessRoot | Out-Null

# Permanent autologon, so a desktop session exists after every boot. The
# unattend AutoLogon block only covers the first logon.
$password = (Get-Content -Raw (Join-Path $AnswerDrive 'password.txt')).Trim()
$winlogon = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
Set-ItemProperty $winlogon -Name AutoAdminLogon -Value '1'
Set-ItemProperty $winlogon -Name DefaultUserName -Value $env:USERNAME
Set-ItemProperty $winlogon -Name DefaultPassword -Value $password
Remove-ItemProperty $winlogon -Name AutoLogonCount -ErrorAction SilentlyContinue
net accounts /maxpwage:unlimited | Out-Null

# Keep the session awake and unlocked so screenshots show the desktop.
powercfg /change standby-timeout-ac 0
powercfg /change monitor-timeout-ac 0
powercfg /change hibernate-timeout-ac 0
$personalization = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Personalization'
New-Item -Force $personalization | Out-Null
Set-ItemProperty $personalization -Name NoLockScreen -Value 1 -Type DWord

# No surprise update reboots in the middle of a test run.
$au = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU'
New-Item -Force $au | Out-Null
Set-ItemProperty $au -Name NoAutoUpdate -Value 1 -Type DWord
Set-ItemProperty $au -Name NoAutoRebootWithLoggedOnUsers -Value 1 -Type DWord

# OpenSSH server: the built-in capability, or the upstream MSI when the
# capability can't be fetched.
$capability = Get-WindowsCapability -Online -Name 'OpenSSH.Server*' | Select-Object -First 1
if ($capability.State -ne 'Installed') {
    try {
        Add-WindowsCapability -Online -Name $capability.Name | Out-Null
    } catch {
        Write-Warning "OpenSSH capability install failed ($_); installing the Win32-OpenSSH MSI instead"
        $release = Invoke-RestMethod 'https://api.github.com/repos/PowerShell/Win32-OpenSSH/releases/latest'
        $asset = $release.assets | Where-Object { $_.name -like 'OpenSSH-Win64-v*.msi' } | Select-Object -First 1
        $msi = Join-Path $env:TEMP $asset.name
        Invoke-WebRequest $asset.browser_download_url -OutFile $msi
        Start-Process msiexec.exe -ArgumentList '/i', "`"$msi`"", '/qn' -Wait
    }
}
Set-Service sshd -StartupType Automatic
Start-Service sshd
if (-not (Get-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -ErrorAction SilentlyContinue)) {
    New-NetFirewallRule -Name 'OpenSSH-Server-In-TCP' -DisplayName 'OpenSSH Server (sshd)' `
        -Enabled True -Direction Inbound -Protocol TCP -Action Allow -LocalPort 22 | Out-Null
}

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
