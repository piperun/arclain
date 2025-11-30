$ErrorActionPreference = "Stop"

Write-Host "Cleaning WASM plugins..."
Write-Host ""

# Get the absolute path to the workspace root
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WorkspaceRoot = Resolve-Path "$ScriptDir\.."
Set-Location $WorkspaceRoot

$PluginsDir = Join-Path $WorkspaceRoot "plugins"

# Remove all existing WASM files in plugins dir (recursively)
Write-Host "Removing old WASM files..."
Get-ChildItem -Path $PluginsDir -Filter "*.wasm" -Recurse | Remove-Item -Force

# Clean each plugin's build artifacts
Write-Host "Cleaning build artifacts..."
$Plugins = Get-ChildItem -Path $PluginsDir -Directory

foreach ($Plugin in $Plugins) {
    $CargoToml = Join-Path $Plugin.FullName "Cargo.toml"
    if (Test-Path $CargoToml) {
        $PluginName = $Plugin.Name
        Write-Host "Cleaning $PluginName..."
        
        Push-Location $Plugin.FullName
        try {
            cargo clean -q
        }
        catch {
            Write-Host "Warning: Failed to clean $PluginName" -ForegroundColor Yellow
        }
        Pop-Location
    }
}

Write-Host ""
Write-Host "Clean complete!" -ForegroundColor Green
