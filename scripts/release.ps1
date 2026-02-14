<#
.SYNOPSIS
    Builds and packages a release of Arclain.

.DESCRIPTION
    1. Updates crate versions via calculate-versions.ps1
    2. Runs full test suite (aborts on failure)
    3. Builds release binary and plugins
    4. Packages everything into a distributable archive

.PARAMETER SkipVersionUpdate
    Skip the version calculation step.

.PARAMETER SkipTests
    Skip the test suite (use for hotfixes only).

.EXAMPLE
    .\release.ps1
    # Full release workflow

.EXAMPLE
    .\release.ps1 -SkipTests
    # Skip tests for hotfix
#>

param(
    [switch]$SkipVersionUpdate,
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot = Split-Path -Parent $ScriptDir

Set-Location $RepoRoot

Write-Host "=== Arclain Release Build ===" -ForegroundColor Cyan
Write-Host "Repository: $RepoRoot" -ForegroundColor Gray
Write-Host ""

# Always use the project's target directory for release builds (avoids ramdisk file locking issues)
# Debug builds still use ramdisk via normal CARGO_TARGET_DIR for speed
$TargetDir = Join-Path $RepoRoot "target"
$env:CARGO_TARGET_DIR = $TargetDir
Write-Host "Using project target directory: $TargetDir" -ForegroundColor Gray

# Step 1: Update versions via cocogitto
if (-not $SkipVersionUpdate) {
    Write-Host "Step 1: Bumping crate versions (cog)..." -ForegroundColor Cyan
    $cogOutput = cog bump --auto --skip-untracked 2>&1
    if ($LASTEXITCODE -ne 0) {
        # cog exits non-zero when there's nothing to bump — that's fine
        if ($cogOutput -match "No conventional commit found") {
            Write-Host "  No version bumps needed" -ForegroundColor Gray
        } else {
            Write-Host $cogOutput
            Write-Error "Version bump failed"
            exit 1
        }
    } else {
        Write-Host $cogOutput
        Write-Host "  Version bump complete" -ForegroundColor Green
    }
} else {
    Write-Host "Step 1: Skipping version update" -ForegroundColor Yellow
}

# Read version from arclain_ui for package naming
$UiCargoPath = Join-Path $RepoRoot "crates\ui\Cargo.toml"
$Version = "0.0.0"
if (Test-Path $UiCargoPath) {
    $content = Get-Content $UiCargoPath -Raw
    if ($content -match 'version\s*=\s*"([^"]+)"') {
        $Version = $matches[1]
    }
}
Write-Host "Building version: $Version" -ForegroundColor Green

# Step 2: Run tests
if (-not $SkipTests) {
    Write-Host "`nStep 2: Running test suite..." -ForegroundColor Cyan
    # Use single-threaded tests for integration tests that access production secrets database
    # This prevents "Database already open. Cannot acquire lock." errors
    cargo test --workspace -- --test-threads=1
    if ($LASTEXITCODE -ne 0) {
        Write-Error "Tests failed! Aborting release."
        exit 1
    }
    Write-Host "All tests passed!" -ForegroundColor Green
} else {
    Write-Host "`nStep 2: Skipping tests" -ForegroundColor Yellow
}

# Step 3: Build release
Write-Host "`nStep 3: Building release binary..." -ForegroundColor Cyan
cargo build --release --package arclain_ui
if ($LASTEXITCODE -ne 0) {
    Write-Error "Release build failed"
    exit 1
}

# Build plugins
Write-Host "Building plugins..." -ForegroundColor Cyan
& "$ScriptDir\build-plugins.ps1"
if ($LASTEXITCODE -ne 0) {
    Write-Warning "Plugin build had issues, continuing..."
}

# Step 4: Package
Write-Host "`nStep 4: Packaging release..." -ForegroundColor Cyan
$ReleaseName = "arclain-$Version-windows-x64"
$ReleaseDir = Join-Path $RepoRoot "release\$ReleaseName"

# Clean and create release directory
if (Test-Path $ReleaseDir) {
    Remove-Item -Recurse -Force $ReleaseDir
}
New-Item -ItemType Directory -Path $ReleaseDir -Force | Out-Null

# Copy binary
$ExePath = Join-Path $TargetDir "release\arclain_ui.exe"
if (Test-Path $ExePath) {
    Copy-Item $ExePath -Destination (Join-Path $ReleaseDir "arclain.exe")
} else {
    Write-Error "Binary not found at $ExePath"
    exit 1
}

# Copy plugins from subdirectories
$PluginsSource = Join-Path $RepoRoot "plugins"
$PluginsDest = Join-Path $ReleaseDir "plugins"
if (Test-Path $PluginsSource) {
    New-Item -ItemType Directory -Path $PluginsDest -Force | Out-Null
    
    # Look for plugin directories (each plugin is in a subdirectory)
    Get-ChildItem -Path $PluginsSource -Directory | ForEach-Object {
        $pluginDir = $_.FullName
        $pluginName = $_.Name
        
        # Skip dead/unused plugins
        if ($pluginName -eq "gstreamer-preview" -or $pluginName -eq "ui-demo") {
            Write-Host "  Skipping unused plugin: $pluginName" -ForegroundColor Gray
            return
        }
        
        # Copy .wasm file if it exists
        $wasmFile = Join-Path $pluginDir "$pluginName.wasm"
        if (Test-Path $wasmFile) {
            Copy-Item $wasmFile -Destination $PluginsDest
            Write-Host "  Copied plugin: $pluginName.wasm" -ForegroundColor Green
        }
        
        # Copy .toml manifest if it exists
        $tomlFile = Join-Path $pluginDir "$pluginName.toml"
        if (Test-Path $tomlFile) {
            Copy-Item $tomlFile -Destination $PluginsDest
        }
    }
}

# Create zip archive
$ZipPath = Join-Path $RepoRoot "release\$ReleaseName.zip"
if (Test-Path $ZipPath) {
    Remove-Item $ZipPath
}
Compress-Archive -Path $ReleaseDir -DestinationPath $ZipPath

Write-Host "`n=== Release Complete ===" -ForegroundColor Green
Write-Host "Package: $ZipPath" -ForegroundColor Cyan
Write-Host "Version: $Version" -ForegroundColor Cyan
