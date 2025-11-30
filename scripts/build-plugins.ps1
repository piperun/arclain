$ErrorActionPreference = "Stop"

# Get the absolute path to the workspace root
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WorkspaceRoot = Resolve-Path "$ScriptDir\.."
Set-Location $WorkspaceRoot

$PluginsDir = Join-Path $WorkspaceRoot "plugins"
$Target = "wasm32-wasip2"

Write-Host "Building WASM plugins..."
Write-Host "Workspace: $WorkspaceRoot"
Write-Host ""

# Check if target is installed
$InstalledTargets = rustup target list --installed
if ($InstalledTargets -notcontains $Target) {
    Write-Host "Target $Target not installed. Installing..."
    rustup target add $Target
}

# Build each plugin
$BuildFailed = $false
$Plugins = Get-ChildItem -Path $PluginsDir -Directory

foreach ($Plugin in $Plugins) {
    $CargoToml = Join-Path $Plugin.FullName "Cargo.toml"
    if (Test-Path $CargoToml) {
        $PluginName = $Plugin.Name
        Write-Host ""
        Write-Host "Building $PluginName..."
        
        Push-Location $Plugin.FullName
        
        # Build for WASM in plugin's own target directory
        try {
            cargo build --target $Target --release --target-dir .
            if ($LASTEXITCODE -ne 0) { throw "Cargo build failed" }
        }
        catch {
            Write-Host "ERROR: Failed to build $PluginName" -ForegroundColor Red
            $BuildFailed = $true
            Pop-Location
            continue
        }
        
        # Determine the WASM file name (replace hyphens with underscores)
        $WasmName = $PluginName -replace "-", "_"
        
        # WASM file is in plugin's local target directory
        $WasmSrc = Join-Path $Target "release\$WasmName.wasm"
        $WasmDest = "$PluginName.wasm"
        
        if (Test-Path $WasmSrc) {
            if ($Target -eq "wasm32-wasip2") {
                Write-Host "Target is wasm32-wasip2, skipping componentization..."
                Copy-Item -Path $WasmSrc -Destination $WasmDest -Force
            } else {
                # Placeholder for componentization if we switch targets later
                Write-Host "Componentizing $PluginName..."
                # wasm-tools component new "$WasmSrc" -o "$WasmDest"
            }
            
            $FileSize = (Get-Item $WasmDest).Length
            Write-Host "Built Component: $WasmDest ($FileSize bytes)" -ForegroundColor Green
        } else {
            Write-Host "WARNING: WASM file not found" -ForegroundColor Yellow
            Write-Host "  Expected: $WasmSrc"
            Write-Host "  Full path: $(Join-Path $Plugin.FullName $WasmSrc)"
        }
        
        Pop-Location
    }
}

Write-Host ""
Write-Host "All plugins processed"

if ($BuildFailed) {
    Write-Host ""
    Write-Host "WARNING: Some plugins failed to build. Check errors above." -ForegroundColor Yellow
    exit 1
}

Write-Host ""
Write-Host "Plugin files created:"
$FoundPlugins = 0
foreach ($Plugin in $Plugins) {
    $PluginName = $Plugin.Name
    $WasmFile = Join-Path $Plugin.FullName "$PluginName.wasm"
    if (Test-Path $WasmFile) {
        $FileSize = (Get-Item $WasmFile).Length
        Write-Host "  - $WasmFile ($FileSize bytes)"
        $FoundPlugins++
    }
}

if ($FoundPlugins -eq 0) {
    Write-Host "  WARNING: No plugin WASM files found!" -ForegroundColor Yellow
    Write-Host "  Check the build output above for errors."
}
