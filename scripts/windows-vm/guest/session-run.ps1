# Runs a command in the logged-on user's desktop session instead of the SSH
# session, which has no desktop: toasts, windows, and screenshots only work
# there. Invoked over SSH by scripts/windows-vm/run-interactive.sh.
# Written for Windows PowerShell 5.1.
param(
    # PowerShell command to run, base64-encoded UTF-8 (empty for screenshot only).
    [string]$CommandBase64 = '',
    # Return once the command starts instead of waiting for it to exit.
    [switch]$Detach,
    # Seconds to wait before the screenshot when detached.
    [int]$DelaySeconds = 10,
    # Seconds to wait for the command to exit when not detached.
    [int]$TimeoutSeconds = 1800,
    # Guest path to write a PNG of the desktop to after the command.
    [string]$Screenshot = ''
)

$ErrorActionPreference = 'Stop'

$harnessRoot = 'C:\residuum-harness'
$jobDir = Join-Path $harnessRoot 'interactive'
$taskName = 'ResiduumHarnessInteractive'

if (-not (Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue |
        Where-Object { $_.UserName -like "*\$env:USERNAME" })) {
    throw "no desktop session for $env:USERNAME. Autologon may be off; reboot the VM with 'vm.sh restart'."
}

# Registers and runs a one-shot task in the interactive session, then waits
# for $doneFile to appear.
function Invoke-InSession([string]$scriptPath, [string]$doneFile, [int]$timeout) {
    Remove-Item $doneFile -ErrorAction SilentlyContinue
    $action = "powershell.exe -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$scriptPath`""
    schtasks /Create /F /TN $taskName /TR $action /SC ONCE /ST 23:59 /RU $env:USERNAME /IT 2>&1 | Out-Null
    if ($LASTEXITCODE) { throw "schtasks /Create failed with exit code $LASTEXITCODE" }
    schtasks /Run /TN $taskName | Out-Null
    if ($LASTEXITCODE) { throw "schtasks /Run failed with exit code $LASTEXITCODE" }
    if ($timeout -gt 0) {
        $deadline = (Get-Date).AddSeconds($timeout)
        while (-not (Test-Path $doneFile)) {
            if ((Get-Date) -gt $deadline) { throw "timed out after ${timeout}s waiting for $doneFile" }
            Start-Sleep -Seconds 1
        }
    }
}

New-Item -ItemType Directory -Force $jobDir | Out-Null
$logFile = Join-Path $jobDir 'output.log'
$exitFile = Join-Path $jobDir 'exit-code'

if ($CommandBase64) {
    $command = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($CommandBase64))
    $jobScript = Join-Path $jobDir 'job.ps1'
    Set-Content -Encoding UTF8 $jobScript @"
Set-Location '$harnessRoot\src'
`$env:CARGO_TARGET_DIR = '$harnessRoot\target'
& { $command } *>&1 | Out-File -Encoding utf8 '$logFile'
`$code = if (`$LASTEXITCODE) { `$LASTEXITCODE } elseif (`$?) { 0 } else { 1 }
Set-Content '$exitFile' `$code
"@
    $wait = if ($Detach) { 0 } else { $TimeoutSeconds }
    Invoke-InSession $jobScript $exitFile $wait
    if ($Detach) { Start-Sleep -Seconds $DelaySeconds }
}

if ($Screenshot) {
    $shotScript = Join-Path $jobDir 'screenshot.ps1'
    $shotDone = Join-Path $jobDir 'screenshot.done'
    Set-Content -Encoding UTF8 $shotScript @"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
`$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen
`$bitmap = New-Object System.Drawing.Bitmap `$bounds.Width, `$bounds.Height
`$graphics = [System.Drawing.Graphics]::FromImage(`$bitmap)
`$graphics.CopyFromScreen(`$bounds.Location, [System.Drawing.Point]::Empty, `$bounds.Size)
`$bitmap.Save('$Screenshot', [System.Drawing.Imaging.ImageFormat]::Png)
Set-Content '$shotDone' ok
"@
    Invoke-InSession $shotScript $shotDone 60
}

if (Test-Path $logFile) { Get-Content $logFile }
if ($CommandBase64 -and -not $Detach) { exit [int](Get-Content $exitFile) }
