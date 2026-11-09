<#
.SYNOPSIS
  Lists the DLLs a PE file imports and fails if any of them is a C/C++
  runtime that a clean Windows install does not ship.

.DESCRIPTION
  The phase-07 portable-exe check: `rizref.exe` must start on a clean
  Windows 10 22H2 machine with no Visual C++ Redistributable installed. That
  holds when the import table names only operating-system DLLs. The static
  CRT (`.cargo/config.toml`) is what makes it so; this script proves it for a
  given build without needing a VM, by reading the PE import directory
  directly (no dumpbin, no Visual Studio).

.EXAMPLE
  pwsh scripts/check-imports.ps1 target/release/rizref.exe
#>
param(
    [Parameter(Mandatory = $true)][string]$Path
)

$ErrorActionPreference = 'Stop'
$bytes = [System.IO.File]::ReadAllBytes((Resolve-Path $Path))

function U16([int]$o) { [BitConverter]::ToUInt16($bytes, $o) }
function U32([int]$o) { [BitConverter]::ToUInt32($bytes, $o) }

$pe = U32 0x3C
if ((U32 $pe) -ne 0x00004550) { throw "$Path is not a PE file" }
$coff = $pe + 4
$sections = U16 ($coff + 2)
$optSize = U16 ($coff + 16)
$opt = $coff + 20
$magic = U16 $opt
$dirBase = if ($magic -eq 0x20B) { $opt + 112 } else { $opt + 96 }
$importRva = U32 ($dirBase + 8)
$delayRva = U32 ($dirBase + 13 * 8)
$sectionTable = $opt + $optSize

function RvaToOffset([uint32]$rva) {
    for ($i = 0; $i -lt $sections; $i++) {
        $s = $sectionTable + 40 * $i
        $va = U32 ($s + 12)
        $raw = U32 ($s + 20)
        $size = [Math]::Max((U32 ($s + 8)), (U32 ($s + 16)))
        if ($rva -ge $va -and $rva -lt $va + $size) { return [int]($rva - $va + $raw) }
    }
    throw "RVA 0x{0:X} is outside every section" -f $rva
}

function ReadName([uint32]$rva) {
    $o = RvaToOffset $rva
    $end = $o
    while ($bytes[$end] -ne 0) { $end++ }
    [System.Text.Encoding]::ASCII.GetString($bytes, $o, $end - $o)
}

$dlls = New-Object System.Collections.Generic.List[string]
if ($importRva -ne 0) {
    $d = RvaToOffset $importRva
    while ((U32 ($d + 12)) -ne 0) { $dlls.Add((ReadName (U32 ($d + 12)))); $d += 20 }
}
if ($delayRva -ne 0) {
    $d = RvaToOffset $delayRva
    while ((U32 ($d + 4)) -ne 0) { $dlls.Add((ReadName (U32 ($d + 4))) + ' (delay)'); $d += 32 }
}

$dlls | Sort-Object -Unique | ForEach-Object { Write-Output "  $_" }
$runtime = $dlls | Where-Object { $_ -match '^(vcruntime|msvcp|msvcr|ucrtbase|api-ms-win-crt-|concrt|vccorlib)' }
if ($runtime) {
    Write-Error ("{0} imports a C/C++ runtime that clean Windows lacks: {1}" -f $Path, ($runtime -join ', '))
    exit 1
}
Write-Output "OK: $Path imports operating-system DLLs only ($($dlls.Count) entries)."
