<#
.SYNOPSIS
  Cold-start benchmark: launch to first presented frame, repeated.

.DESCRIPTION
  Runs `rizref.exe --exit-after-first-frame` -Runs times. Each run opens the
  window, initialises the GPU, presents one frame of the empty canvas (the
  start view), prints its startup waterfall as JSON and exits. The script
  reports the median and the worst of every milestone; the budget is
  first_frame < 300 ms.

  The first run after a build pays disk and shader-cache warm-up and is
  reported but excluded from the median ("cold" vs "warm" launches).

.EXAMPLE
  pwsh scripts/bench-startup.ps1 -Exe target/release/rizref.exe -Runs 11
#>
param(
    [string]$Exe = 'target/release/rizref.exe',
    [int]$Runs = 11,
    [string[]]$Arguments = @()
)

$ErrorActionPreference = 'Stop'
$exePath = (Resolve-Path $Exe).Path
$results = @()
for ($i = 0; $i -lt $Runs; $i++) {
    $json = & $exePath --exit-after-first-frame @Arguments | Select-Object -Last 1
    if ($LASTEXITCODE -ne 0 -or -not $json) { throw "run $i failed (exit $LASTEXITCODE)" }
    $results += ($json | ConvertFrom-Json)
}

$milestones = 'logging_ms', 'event_loop_ms', 'window_ms', 'gpu_ms', 'first_frame_ms'
Write-Output ("first run (cold): first_frame {0:N1} ms" -f $results[0].first_frame_ms)
$warm = $results | Select-Object -Skip 1
foreach ($m in $milestones) {
    $values = @($warm | ForEach-Object { [double]$_.$m } | Sort-Object)
    if ($values.Count -eq 0) { continue }
    $median = $values[[int][Math]::Floor($values.Count / 2)]
    Write-Output ("{0,-16} median {1,7:N1} ms   worst {2,7:N1} ms   ({3} warm runs)" -f $m, $median, $values[-1], $values.Count)
}
