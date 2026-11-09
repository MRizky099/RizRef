<#
.SYNOPSIS
  Builds and packages a RizRef release: the portable zip and, when Inno Setup
  is installed, the per-user installer.

.DESCRIPTION
  1. `cargo build --release` for `rizref` (the exe) and `rizref-thumb` (the
     optional Explorer thumbnail DLL).
  2. Checks the exe's import table: OS DLLs only (scripts/check-imports.ps1).
  3. Signs both binaries when a certificate is configured (see -Sign).
  4. Stages `dist/RizRef-<version>-x64/` and zips it — the portable build:
     `rizref.exe` runs on its own; `rizref_thumb.dll` next to it adds
     Explorer thumbnails after `rizref register`.
  5. Compiles `packaging/rizref.iss` with ISCC.exe when it is available.

  Signing is a hook, not a secret store. With -Sign the script expects
  `RIZREF_SIGN_PFX` (path to a .pfx) and `RIZREF_SIGN_PASSWORD` in the
  environment and `signtool.exe` on PATH (Windows SDK); the release workflow
  fills them from repository secrets when those exist. Without them it skips
  signing and says so. Nothing secret is ever written to the repository.

.EXAMPLE
  pwsh packaging/package.ps1
  pwsh packaging/package.ps1 -Sign -SkipBuild
#>
param(
    [switch]$Sign,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
Write-Output "RizRef $version"

if (-not $SkipBuild) {
    cargo build --release -p rizref -p rizref-thumb
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}
$exe = Join-Path $root 'target/release/rizref.exe'
$dll = Join-Path $root 'target/release/rizref_thumb.dll'

& (Join-Path $root 'scripts/check-imports.ps1') $exe
if ($LASTEXITCODE -ne 0) { throw 'rizref.exe imports a non-OS runtime' }
& (Join-Path $root 'scripts/check-imports.ps1') $dll
if ($LASTEXITCODE -ne 0) { throw 'rizref_thumb.dll imports a non-OS runtime' }

function Invoke-Sign([string]$file) {
    $pfx = $env:RIZREF_SIGN_PFX
    $password = $env:RIZREF_SIGN_PASSWORD
    if (-not $pfx -or -not (Test-Path $pfx)) {
        Write-Warning "signing skipped for $file : RIZREF_SIGN_PFX is not set (no certificate yet)"
        return
    }
    $signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if (-not $signtool) { throw 'signtool.exe is not on PATH (install the Windows SDK)' }
    & $signtool.Source sign /fd SHA256 /td SHA256 /tr 'http://timestamp.digicert.com' `
        /f $pfx /p $password $file
    if ($LASTEXITCODE -ne 0) { throw "signing $file failed" }
}

if ($Sign) {
    Invoke-Sign $exe
    Invoke-Sign $dll
}

$name = "RizRef-$version-x64"
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item $exe, $dll $stage
Copy-Item (Join-Path $root 'packaging/README.txt'), (Join-Path $root 'LICENSE-MIT'), (Join-Path $root 'LICENSE-APACHE') $stage
$zip = Join-Path $dist "$name-portable.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip
$exeSize = (Get-Item $exe).Length
Write-Output ("rizref.exe       {0:N0} bytes ({1:N2} MB)" -f $exeSize, ($exeSize / 1MB))
Write-Output ("rizref_thumb.dll {0:N0} bytes" -f (Get-Item $dll).Length)
Write-Output "portable zip:    $zip"

$iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue
if (-not $iscc) {
    $default = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
    if (Test-Path $default) { $iscc = Get-Item $default }
}
if ($iscc) {
    $path = if ($iscc -is [System.Management.Automation.CommandInfo]) { $iscc.Source } else { $iscc.FullName }
    & $path "/DAppVersion=$version" "/DStageDir=$stage" "/O$dist" (Join-Path $root 'packaging/rizref.iss')
    if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }
    $setup = Join-Path $dist "$name-setup.exe"
    if ($Sign) { Invoke-Sign $setup }
    Write-Output "installer:       $setup"
} else {
    Write-Warning 'Inno Setup (ISCC.exe) not found: installer skipped. Install Inno Setup 6 to build it.'
}
