<#
.SYNOPSIS
    Updates all cargo dependencies in the monorepo.

.DESCRIPTION
    This script provides several options for updating dependencies:
    
    1. Check for outdated dependencies (default)
    2. Update Cargo.lock to latest compatible versions (-Update)
    3. Upgrade Cargo.toml version constraints (-Upgrade, requires cargo-edit)

.PARAMETER Update
    Run 'cargo update' to update Cargo.lock to latest compatible versions.

.PARAMETER Upgrade
    Run 'cargo upgrade' to update version constraints in Cargo.toml files.
    Requires cargo-edit: cargo install cargo-edit

.PARAMETER Incompatible
    When used with -Upgrade, also upgrade to incompatible (breaking) versions.

.PARAMETER DryRun
    Show what would be updated without making changes.

.EXAMPLE
    .\update-deps.ps1
    # Check for outdated dependencies

.EXAMPLE
    .\update-deps.ps1 -Update
    # Update Cargo.lock to latest compatible versions

.EXAMPLE
    .\update-deps.ps1 -Upgrade
    # Update Cargo.toml version constraints (compatible only)

.EXAMPLE
    .\update-deps.ps1 -Upgrade -Incompatible
    # Update Cargo.toml to latest versions (including breaking changes)
#>

param(
    [switch]$Update,
    [switch]$Upgrade,
    [switch]$Incompatible,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

# Get repository root
$RepoRoot = git rev-parse --show-toplevel 2>$null
if (-not $RepoRoot) {
    Write-Error "Not in a git repository"
    exit 1
}

Set-Location $RepoRoot

Write-Host "=== Cargo Dependency Manager ===" -ForegroundColor Cyan
Write-Host "Workspace: $RepoRoot" -ForegroundColor Gray
Write-Host ""

# Check if we have a workspace
$workspaceCargo = Join-Path $RepoRoot "Cargo.toml"
if (-not (Test-Path $workspaceCargo)) {
    Write-Error "No Cargo.toml found at repository root"
    exit 1
}

$isWorkspace = Select-String -Path $workspaceCargo -Pattern '^\[workspace\]' -Quiet
if ($isWorkspace) {
    Write-Host "Detected: Cargo Workspace" -ForegroundColor Green
} else {
    Write-Host "Detected: Single crate (not a workspace)" -ForegroundColor Yellow
}
Write-Host ""

# Function to check if a command exists
function Test-Command {
    param([string]$Command)
    $null -ne (Get-Command $Command -ErrorAction SilentlyContinue)
}

# Check for outdated dependencies
if (-not $Update -and -not $Upgrade) {
    Write-Host "Checking for outdated dependencies..." -ForegroundColor Cyan
    Write-Host "(This may take a moment to fetch from crates.io)" -ForegroundColor Gray
    Write-Host ""
    
    # Check if cargo-outdated is installed
    $hasOutdated = Test-Command "cargo-outdated"
    
    if ($hasOutdated) {
        Write-Host "Using cargo-outdated for detailed report:" -ForegroundColor Yellow
        $ErrorActionPreference = "Continue"
        cargo outdated --workspace
        $ErrorActionPreference = "Stop"
    } else {
        Write-Host "Note: Install 'cargo-outdated' for a detailed outdated report:" -ForegroundColor Yellow
        Write-Host "  cargo install cargo-outdated" -ForegroundColor Gray
        Write-Host ""
        Write-Host "Using 'cargo update --dry-run' to check for updates:" -ForegroundColor Cyan
        # Temporarily allow stderr (cargo writes progress to stderr)
        $ErrorActionPreference = "Continue"
        $output = cargo update --dry-run 2>&1
        $ErrorActionPreference = "Stop"
        
        foreach ($line in $output) {
            $text = $line.ToString()
            if ($text -match "Updating|Removing|Adding") {
                Write-Host $text -ForegroundColor Yellow
            } else {
                Write-Host $text -ForegroundColor Gray
            }
        }
    }
    
    Write-Host ""
    Write-Host "To update dependencies, run:" -ForegroundColor Cyan
    Write-Host "  .\update-deps.ps1 -Update     # Update Cargo.lock (safe)" -ForegroundColor Gray
    Write-Host "  .\update-deps.ps1 -Upgrade    # Update Cargo.toml constraints" -ForegroundColor Gray
    exit 0
}

# Update Cargo.lock
if ($Update) {
    Write-Host "Updating Cargo.lock..." -ForegroundColor Cyan
    
    if ($DryRun) {
        Write-Host "(Dry run - no changes will be made)" -ForegroundColor Yellow
        cargo update --dry-run
    } else {
        cargo update
        
        if ($LASTEXITCODE -eq 0) {
            Write-Host ""
            Write-Host "Cargo.lock updated successfully!" -ForegroundColor Green
            Write-Host ""
            Write-Host "Next steps:" -ForegroundColor Cyan
            Write-Host "  1. Run 'cargo check' to verify build" -ForegroundColor Gray
            Write-Host "  2. Run tests to verify compatibility" -ForegroundColor Gray
            Write-Host "  3. Commit the updated Cargo.lock" -ForegroundColor Gray
        } else {
            Write-Host "Update failed!" -ForegroundColor Red
            exit 1
        }
    }
}

# Upgrade Cargo.toml constraints
if ($Upgrade) {
    # Check if cargo-edit is installed
    $hasEdit = $null -ne (cargo upgrade --version 2>$null)
    
    if (-not $hasEdit) {
        Write-Host "cargo-edit is required for upgrading Cargo.toml constraints." -ForegroundColor Red
        Write-Host ""
        Write-Host "Install it with:" -ForegroundColor Cyan
        Write-Host "  cargo install cargo-edit" -ForegroundColor Gray
        Write-Host ""
        Write-Host "Or use -Update to just update Cargo.lock (doesn't require cargo-edit)" -ForegroundColor Yellow
        exit 1
    }
    
    Write-Host "Upgrading Cargo.toml version constraints..." -ForegroundColor Cyan
    
    $upgradeArgs = @("upgrade", "--workspace")
    
    if ($Incompatible) {
        $upgradeArgs += "--incompatible"
        Write-Host "(Including incompatible/breaking version updates)" -ForegroundColor Yellow
    }
    
    if ($DryRun) {
        $upgradeArgs += "--dry-run"
        Write-Host "(Dry run - no changes will be made)" -ForegroundColor Yellow
    }
    
    & cargo @upgradeArgs
    
    if ($LASTEXITCODE -eq 0 -and -not $DryRun) {
        Write-Host ""
        Write-Host "Cargo.toml files upgraded successfully!" -ForegroundColor Green
        Write-Host ""
        Write-Host "Next steps:" -ForegroundColor Cyan
        Write-Host "  1. Review changes with 'git diff'" -ForegroundColor Gray
        Write-Host "  2. Run 'cargo check' to verify build" -ForegroundColor Gray
        Write-Host "  3. Run tests to verify compatibility" -ForegroundColor Gray
        Write-Host "  4. Commit the updated Cargo.toml files" -ForegroundColor Gray
    }
}

Write-Host ""
Write-Host "Done!" -ForegroundColor Cyan
