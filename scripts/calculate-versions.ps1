<#
.SYNOPSIS
    Calculates semantic version numbers for each crate in the monorepo based on git history.

.DESCRIPTION
    This script analyzes git commit history to determine appropriate version numbers
    for each crate. It uses a smart versioning scheme:
    
    - Major: 0 (pre-1.0) or based on explicit version tags
    - Minor: Increments based on "significant" commits (breaking changes, features)
    - Patch: Calculated from total commits affecting the crate, scaled down
    
    The script looks for existing version tags in the format: <crate-name>-v<version>
    If found, it uses that as the base and counts commits since.

.PARAMETER UpdateCargo
    If specified, updates each crate's Cargo.toml with the calculated version.

.PARAMETER ShowDetails
    Show detailed commit information for each crate.

.EXAMPLE
    .\calculate-versions.ps1
    # Shows calculated versions for all crates

.EXAMPLE
    .\calculate-versions.ps1 -UpdateCargo
    # Updates Cargo.toml files with calculated versions
#>

param(
    [switch]$UpdateCargo,
    [switch]$ShowDetails
)

$ErrorActionPreference = "Stop"

# Configuration
$BaseVersion = @{
    Major = 0
    Minor = 1
}

# How many commits per patch increment (prevents version explosion)
$CommitsPerPatch = 10

# Keywords that indicate a minor version bump (in commit messages)
$MinorBumpKeywords = @(
    "feat:",
    "feature:",
    "BREAKING:",
    "breaking:",
    "!:"  # Conventional commits breaking change indicator
)

# Get repository root
$RepoRoot = git rev-parse --show-toplevel 2>$null
if (-not $RepoRoot) {
    Write-Error "Not in a git repository"
    exit 1
}

Set-Location $RepoRoot

Write-Host "Calculating versions for Arclain monorepo..." -ForegroundColor Cyan
Write-Host "Repository: $RepoRoot" -ForegroundColor Gray
Write-Host ""

# Find all crates (directories containing Cargo.toml)
$Crates = @()

# Check crates/ directory
$CratesDir = Join-Path $RepoRoot "crates"
if (Test-Path $CratesDir) {
    Get-ChildItem -Path $CratesDir -Directory | ForEach-Object {
        $cargoPath = Join-Path $_.FullName "Cargo.toml"
        if (Test-Path $cargoPath) {
            $Crates += @{
                Name = $_.Name
                Path = $_.FullName
                RelativePath = "crates/$($_.Name)"
                CargoToml = $cargoPath
            }
        }
    }
}

# Check plugins/ directory
$PluginsDir = Join-Path $RepoRoot "plugins"
if (Test-Path $PluginsDir) {
    Get-ChildItem -Path $PluginsDir -Directory | ForEach-Object {
        $cargoPath = Join-Path $_.FullName "Cargo.toml"
        if (Test-Path $cargoPath) {
            $Crates += @{
                Name = $_.Name
                Path = $_.FullName
                RelativePath = "plugins/$($_.Name)"
                CargoToml = $cargoPath
                IsPlugin = $true
            }
        }
    }
}

# Check for root crate
$RootCargo = Join-Path $RepoRoot "Cargo.toml"
if ((Test-Path $RootCargo) -and (Select-String -Path $RootCargo -Pattern '^\[package\]' -Quiet)) {
    $Crates += @{
        Name = "arclain"
        Path = $RepoRoot
        RelativePath = "."
        CargoToml = $RootCargo
        IsRoot = $true
    }
}

Write-Host "Found $($Crates.Count) crates:" -ForegroundColor Yellow
$Crates | ForEach-Object { Write-Host "  - $($_.Name) ($($_.RelativePath))" -ForegroundColor Gray }
Write-Host ""

# Function to get current version from Cargo.toml (only from [package] section)
function Get-CurrentVersion {
    param([string]$CargoPath)
    
    $lines = Get-Content $CargoPath
    $inPackageSection = $false
    
    foreach ($line in $lines) {
        # Track which section we're in
        if ($line -match '^\s*\[') {
            $inPackageSection = $line -match '^\s*\[package\]'
        }
        
        # Only read version if we're in [package] section
        if ($inPackageSection -and $line -match '^\s*version\s*=\s*"([^"]+)"') {
            return $matches[1]
        }
    }
    
    return "0.1.0"
}

# Function to find version tag for a crate
function Get-VersionTag {
    param([string]$CrateName)
    
    # Look for tags like: crate-name-v0.1.0 or v0.1.0-crate-name
    $tags = git tag --list "$CrateName-v*" 2>$null
    if (-not $tags) {
        $tags = git tag --list "v*-$CrateName" 2>$null
    }
    
    if ($tags) {
        # Get the latest tag
        $latestTag = $tags | Sort-Object -Descending | Select-Object -First 1
        return $latestTag
    }
    
    return $null
}

# Function to count commits affecting a path
function Get-CommitCount {
    param(
        [string]$Path,
        [string]$Since = $null
    )
    
    $args = @("rev-list", "--count", "HEAD")
    if ($Since) {
        $args += "$Since..HEAD"
    }
    $args += "--"
    $args += $Path
    
    $count = git @args 2>$null
    if ($count) {
        return [int]$count
    }
    return 0
}

# Function to count "minor" commits (features, breaking changes)
function Get-MinorCommitCount {
    param(
        [string]$Path,
        [string]$Since = $null
    )
    
    $args = @("log", "--oneline")
    if ($Since) {
        $args += "$Since..HEAD"
    }
    $args += "--"
    $args += $Path
    
    $commits = git @args 2>$null
    if (-not $commits) {
        return 0
    }
    
    $minorCount = 0
    foreach ($commit in $commits) {
        foreach ($keyword in $MinorBumpKeywords) {
            if ($commit -match [regex]::Escape($keyword)) {
                $minorCount++
                break
            }
        }
    }
    
    return $minorCount
}

# Function to get the first commit date for a path (for versioning baseline)
function Get-FirstCommitDate {
    param([string]$Path)
    
    $date = git log --reverse --format="%ci" -- $Path 2>$null | Select-Object -First 1
    if ($date) {
        return [datetime]::Parse($date.Substring(0, 10))
    }
    return $null
}

# Calculate version for each crate
$Results = @()

foreach ($crate in $Crates) {
    Write-Host "Analyzing: $($crate.Name)" -ForegroundColor Cyan
    
    $currentVersion = Get-CurrentVersion -CargoPath $crate.CargoToml
    $versionTag = Get-VersionTag -CrateName $crate.Name
    
    # Count commits
    $totalCommits = Get-CommitCount -Path $crate.RelativePath
    $minorCommits = Get-MinorCommitCount -Path $crate.RelativePath
    
    # Calculate version
    $major = $BaseVersion.Major
    $minor = $BaseVersion.Minor
    $patch = 0
    
    if ($versionTag) {
        # Parse version from tag
        if ($versionTag -match 'v?(\d+)\.(\d+)\.(\d+)') {
            $major = [int]$matches[1]
            $minor = [int]$matches[2]
            $baseFromTag = [int]$matches[3]
            
            # Count commits since tag
            $commitsSinceTag = Get-CommitCount -Path $crate.RelativePath -Since $versionTag
            $patch = $baseFromTag + [math]::Floor($commitsSinceTag / $CommitsPerPatch)
        }
        Write-Host "  Found tag: $versionTag" -ForegroundColor Gray
    }
    else {
        # No tag - calculate from scratch
        # Minor bumps based on feature/breaking commits (capped)
        $minorBumps = [math]::Min([math]::Floor($minorCommits / 5), 10)  # Cap at 10 minor bumps
        $minor = $BaseVersion.Minor + $minorBumps
        
        # Patch based on total commits, scaled down
        $patch = [math]::Floor($totalCommits / $CommitsPerPatch)
    }
    
    # Ensure patch doesn't exceed 99 (roll into minor)
    while ($patch -ge 100) {
        $minor++
        $patch -= 100
    }
    
    # Ensure minor doesn't exceed 99 (roll into major) - unlikely but safe
    while ($minor -ge 100) {
        $major++
        $minor -= 100
    }
    
    $calculatedVersion = "$major.$minor.$patch"
    
    $result = [PSCustomObject]@{
        Name = $crate.Name
        Path = $crate.RelativePath
        CurrentVersion = $currentVersion
        CalculatedVersion = $calculatedVersion
        TotalCommits = $totalCommits
        MinorCommits = $minorCommits
        Tag = if ($versionTag) { $versionTag } else { "-" }
        NeedsUpdate = $currentVersion -ne $calculatedVersion
        CargoToml = $crate.CargoToml
    }
    
    $Results += $result
    
    if ($ShowDetails) {
        Write-Host "  Total commits: $totalCommits" -ForegroundColor Gray
        Write-Host "  Feature/Breaking commits: $minorCommits" -ForegroundColor Gray
    }
    
    $versionColor = if ($result.NeedsUpdate) { "Yellow" } else { "Green" }
    Write-Host "  Current: $currentVersion -> Calculated: $calculatedVersion" -ForegroundColor $versionColor
    Write-Host ""
}

# Summary
Write-Host "`n" + ("=" * 60) -ForegroundColor Cyan
Write-Host "VERSION SUMMARY" -ForegroundColor Cyan
Write-Host ("=" * 60) -ForegroundColor Cyan

$Results | Format-Table -Property Name, CurrentVersion, CalculatedVersion, TotalCommits, NeedsUpdate -AutoSize

# Show which need updates
$needsUpdate = $Results | Where-Object { $_.NeedsUpdate }
if ($needsUpdate) {
    Write-Host "`nCrates needing version update:" -ForegroundColor Yellow
    $needsUpdate | ForEach-Object {
        Write-Host "  $($_.Name): $($_.CurrentVersion) -> $($_.CalculatedVersion)" -ForegroundColor Yellow
    }
}
else {
    Write-Host "`nAll crates are up to date!" -ForegroundColor Green
}

# Update Cargo.toml files if requested
if ($UpdateCargo -and $needsUpdate) {
    Write-Host "`nUpdating Cargo.toml files..." -ForegroundColor Cyan
    
    foreach ($result in $needsUpdate) {
        $lines = Get-Content $result.CargoToml
        $inPackageSection = $false
        $versionUpdated = $false
        $newLines = @()
        
        foreach ($line in $lines) {
            # Track which section we're in
            if ($line -match '^\s*\[') {
                $inPackageSection = $line -match '^\s*\[package\]'
            }
            
            # Only update version if we're in [package] section and haven't updated yet
            if ($inPackageSection -and -not $versionUpdated -and $line -match '^(\s*version\s*=\s*")[^"]+(".*$)') {
                $newLines += "$($matches[1])$($result.CalculatedVersion)$($matches[2])"
                $versionUpdated = $true
            }
            else {
                $newLines += $line
            }
        }
        
        if ($versionUpdated) {
            $newLines | Set-Content -Path $result.CargoToml
            Write-Host "  Updated: $($result.Path)/Cargo.toml" -ForegroundColor Green
        }
        else {
            Write-Host "  Warning: Could not find version in [package] section of $($result.Path)/Cargo.toml" -ForegroundColor Yellow
        }
    }
    
    Write-Host "`nDone! Remember to:" -ForegroundColor Cyan
    Write-Host "  1. Review the changes with 'git diff'" -ForegroundColor Gray
    Write-Host "  2. Run 'cargo check' to verify" -ForegroundColor Gray
    Write-Host "  3. Commit the version updates" -ForegroundColor Gray
}
elseif ($UpdateCargo -and -not $needsUpdate) {
    Write-Host "`nNo updates needed." -ForegroundColor Green
}

# Output as JSON for programmatic use
$jsonOutput = $Results | ConvertTo-Json -Depth 3
$jsonPath = Join-Path $RepoRoot ".versions.json"
$jsonOutput | Set-Content $jsonPath
Write-Host "`nVersion data saved to: $jsonPath" -ForegroundColor Gray
