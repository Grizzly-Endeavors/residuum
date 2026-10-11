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
    [string]$Screenshot = '',
    # Stop a detached command and everything it started, then exit.
    [switch]$Stop,
    # Fail if a detached command is still running, then exit. The host checks
    # this before syncing, since the running command holds the source tree.
    [switch]$CheckIdle
)

$ErrorActionPreference = 'Stop'

$harnessRoot = 'C:\residuum-harness'
$jobDir = Join-Path $harnessRoot 'interactive'
# Separate tasks, because Task Scheduler won't start a task that is already
# running: a screenshot taken while a detached command runs needs its own.
$jobTask = 'ResiduumHarnessJob'
$screenshotTask = 'ResiduumHarnessScreenshot'

# The job's PowerShell process. Ending the scheduled task only kills the
# conhost wrapper and leaves this running, so liveness and stopping go by it.
function Get-JobProcess {
    Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe'" |
        Where-Object { $_.CommandLine -like '*\interactive\job.ps1*' }
}

if ($Stop) {
    $running = @(Get-JobProcess)
    if (-not $running) {
        Write-Output 'no detached command is running'
        exit 0
    }
    foreach ($process in $running) {
        taskkill /F /T /PID $process.ProcessId | Out-Null
    }
    schtasks /End /TN $jobTask 2>&1 | Out-Null
    if (@(Get-JobProcess)) { throw 'the detached command is still running after taskkill' }
    Write-Output 'stopped the detached command'
    exit 0
}

if ($CheckIdle -or $CommandBase64) {
    # A new job would overwrite the files a running one is still using.
    if (@(Get-JobProcess)) {
        throw 'a detached command from an earlier run is still running. Stop it with: scripts/windows-vm/run-interactive.sh --stop'
    }
    if ($CheckIdle) { exit 0 }
}

if (-not (Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue |
        Where-Object { $_.UserName -like "*\$env:USERNAME" })) {
    throw "no desktop session for $env:USERNAME. Autologon may be off; reboot the VM with 'vm.sh restart'."
}

# Registers and runs a one-shot task in the interactive session, then waits
# for $doneFile to appear.
function Invoke-InSession([string]$taskName, [string]$scriptPath, [string]$doneFile, [int]$timeout) {
    Remove-Item $doneFile -ErrorAction SilentlyContinue
    # Windows Terminal, the default console host on Windows 11, ignores
    # -WindowStyle Hidden and would leave a window over the desktop under test.
    $action = "conhost.exe --headless powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$scriptPath`""
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
    Remove-Item $logFile -ErrorAction SilentlyContinue
    $command = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($CommandBase64))
    $jobScript = Join-Path $jobDir 'job.ps1'
    Set-Content -Encoding UTF8 $jobScript @"
Set-Location '$harnessRoot\src'
`$env:CARGO_TARGET_DIR = '$harnessRoot\target'
`$global:LASTEXITCODE = `$null
`$script:succeeded = `$true
& {
$command
`$script:succeeded = `$?
} *>&1 | Out-File -Encoding utf8 '$logFile'
`$code = if (`$null -ne `$LASTEXITCODE) { `$LASTEXITCODE } elseif (`$script:succeeded) { 0 } else { 1 }
Set-Content '$exitFile' `$code
"@
    $wait = if ($Detach) { 0 } else { $TimeoutSeconds }
    Invoke-InSession $jobTask $jobScript $exitFile $wait
    if ($Detach) { Start-Sleep -Seconds $DelaySeconds }
}

if ($Screenshot) {
    Remove-Item $Screenshot -ErrorAction SilentlyContinue
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
    Invoke-InSession $screenshotTask $shotScript $shotDone 60
}

if ($CommandBase64 -and (Test-Path $logFile)) { Get-Content $logFile }
if ($CommandBase64 -and -not $Detach) { exit [int](Get-Content $exitFile) }
