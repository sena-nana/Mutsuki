$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$Results = @()

function Invoke-MutsukiCheck {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory,
        [Parameter(Mandatory = $true)]
        [string]$Command,
        [string[]]$Arguments = @()
    )

    $started = Get-Date
    Write-Host ""
    Write-Host "==> $Name"
    Write-Host "cwd: $WorkingDirectory"
    Write-Host "cmd: $Command $($Arguments -join ' ')"

    Push-Location -LiteralPath $WorkingDirectory
    try {
        & $Command @Arguments
        $exitCode = if ($null -eq $LASTEXITCODE) { 0 } else { $LASTEXITCODE }
    }
    catch {
        Write-Host $_
        $exitCode = 1
    }
    finally {
        Pop-Location
    }

    $durationMs = [int]((Get-Date) - $started).TotalMilliseconds
    $script:Results += [pscustomobject]@{
        Name = $Name
        ExitCode = $exitCode
        DurationMs = $durationMs
    }

    if ($exitCode -ne 0) {
        Write-Host "FAILED: $Name exited with $exitCode after ${durationMs}ms"
    }
    else {
        Write-Host "OK: $Name completed in ${durationMs}ms"
    }
}

Invoke-MutsukiCheck `
    -Name "cargo fmt --check" `
    -WorkingDirectory $RepoRoot `
    -Command "cargo" `
    -Arguments @("fmt", "--check")

Invoke-MutsukiCheck `
    -Name "cargo test" `
    -WorkingDirectory $RepoRoot `
    -Command "cargo" `
    -Arguments @("test")

Write-Host ""
Write-Host "==> Summary"
$Results | Format-Table -AutoSize

$failed = @($Results | Where-Object { $_.ExitCode -ne 0 })
if ($failed.Count -gt 0) {
    Write-Host ""
    Write-Host "Mutsuki runtime checks failed: $($failed.Name -join ', ')"
    exit 1
}

Write-Host ""
Write-Host "Mutsuki runtime checks passed."
exit 0
