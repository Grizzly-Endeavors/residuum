# Installs the residuum build toolchain in the Windows VM. Safe to re-run:
# anything already present is left alone unless -Upgrade is given.
# Run from the host with scripts/windows-vm/vm.sh provision.
# Written for Windows PowerShell 5.1.
param(
    [switch]$Upgrade
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$InformationPreference = 'Continue'
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$harnessRoot = 'C:\residuum-harness'
$downloads = Join-Path $harnessRoot 'downloads'
New-Item -ItemType Directory -Force $downloads | Out-Null

function Import-RegistryPath {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $user = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:Path = "$machine;$user"
}

function Add-MachinePath([string]$dir) {
    $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    if (($machine -split ';') -notcontains $dir) {
        [Environment]::SetEnvironmentVariable('Path', "$machine;$dir", 'Machine')
    }
    Import-RegistryPath
}

function Test-Command([string]$name) {
    [bool](Get-Command $name -ErrorAction SilentlyContinue)
}

function Get-LatestGitHubAsset([string]$repo, [scriptblock]$match) {
    $release = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest"
    $asset = $release.assets | Where-Object $match | Select-Object -First 1
    if (-not $asset) { throw "no matching asset in the latest $repo release ($($release.tag_name))" }
    $asset
}

function Save-Download([string]$url, [string]$name) {
    $path = Join-Path $downloads $name
    Write-Information "    downloading $url"
    Invoke-WebRequest $url -OutFile $path
    $path
}

Import-RegistryPath

# Builds are much slower when Defender scans every object file cargo writes.
Add-MpPreference -ExclusionPath $harnessRoot, "$env:USERPROFILE\.cargo", "$env:USERPROFILE\.rustup", 'C:\winlibs'

Write-Information '==> Git'
if ($Upgrade -or -not (Test-Command git)) {
    $asset = Get-LatestGitHubAsset 'git-for-windows/git' { $_.name -match '^Git-[\d.]+-64-bit\.exe$' }
    $installer = Save-Download $asset.browser_download_url $asset.name
    Start-Process $installer -ArgumentList '/VERYSILENT', '/NORESTART', '/SUPPRESSMSGBOXES' -Wait
    Import-RegistryPath
}

Write-Information '==> Node.js (current LTS)'
if ($Upgrade -or -not (Test-Command node)) {
    $lts = (Invoke-RestMethod 'https://nodejs.org/dist/index.json') | Where-Object { $_.lts } | Select-Object -First 1
    $msiName = "node-$($lts.version)-x64.msi"
    $msi = Save-Download "https://nodejs.org/dist/$($lts.version)/$msiName" $msiName
    Start-Process msiexec.exe -ArgumentList '/i', "`"$msi`"", '/qn', '/norestart' -Wait
    Import-RegistryPath
}

# The gnu Rust toolchain bundles a linker but no C compiler, and this build
# compiles C through the cc crate (bundled SQLite, zstd, ring, aws-lc). WinLibs
# ships GCC with CMake and NASM; the msvcrt build matches the runtime the
# release cross-compiler targets.
Write-Information '==> MinGW-w64 GCC (WinLibs, msvcrt)'
$mingwBin = 'C:\winlibs\mingw64\bin'
if ($Upgrade -or -not (Test-Path (Join-Path $mingwBin 'gcc.exe'))) {
    $releases = Invoke-RestMethod 'https://api.github.com/repos/brechtsanders/winlibs_mingw/releases?per_page=20'
    $asset = $releases |
        Where-Object { -not $_.prerelease } |
        ForEach-Object { $_.assets } |
        Where-Object { $_.name -match '^winlibs-x86_64-posix-seh-gcc-[\d.]+-mingw-w64msvcrt-[\d.]+-r\d+\.zip$' } |
        Select-Object -First 1
    if (-not $asset) { throw 'no WinLibs x86_64 msvcrt release found' }
    $zip = Save-Download $asset.browser_download_url $asset.name
    Remove-Item -Recurse -Force 'C:\winlibs' -ErrorAction SilentlyContinue
    Expand-Archive $zip -DestinationPath 'C:\winlibs'
}
Add-MachinePath $mingwBin

Write-Information '==> Rust (stable, x86_64-pc-windows-gnu host)'
if (Test-Command rustup) {
    if ($Upgrade) { rustup update stable }
} else {
    $rustupInit = Save-Download 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-gnu/rustup-init.exe' 'rustup-init.exe'
    & $rustupInit -y --default-host x86_64-pc-windows-gnu --default-toolchain stable --profile minimal --component clippy --component rustfmt
    if ($LASTEXITCODE) { throw "rustup-init failed with exit code $LASTEXITCODE" }
    Import-RegistryPath
}

Write-Information '==> Installed versions'
git --version
node --version
gcc --version | Select-Object -First 1
cmake --version | Select-Object -First 1
nasm -v
rustc --version
cargo --version
