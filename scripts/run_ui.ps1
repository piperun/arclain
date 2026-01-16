$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$ProjectRoot = Split-Path -Parent $ScriptDir
$ConfigFile = Join-Path $ScriptDir "logging_config.json"

# Default RUST_LOG logic if config is missing
$RustLog = "arclain=debug,info"

if (Test-Path $ConfigFile) {
    Write-Host "Reading logging config from $ConfigFile" -ForegroundColor Cyan
    try {
        $Config = Get-Content $ConfigFile -Raw | ConvertFrom-Json
        
        # Start with default level
        $Parts = @($Config.default_level)
        
        # Add specific filters
        if ($Config.filters) {
            $Config.filters.PSObject.Properties | ForEach-Object {
                $Parts += "$($_.Name)=$($_.Value)"
            }
        }
        
        $RustLog = $Parts -join ","
    } catch {
        Write-Warning "Failed to parse logging_config.json: $_"
    }
} else {
    Write-Warning "logging_config.json not found, using default: $RustLog"
}

Write-Host "Setting RUST_LOG=$RustLog" -ForegroundColor Green
$env:RUST_LOG = $RustLog

Set-Location $ProjectRoot
cargo ui $args
