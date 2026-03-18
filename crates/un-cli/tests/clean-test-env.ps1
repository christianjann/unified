# Clean up test environment for unified repo manager

$ErrorActionPreference = "Stop"

Write-Host "Cleaning up test environment..."

# Remove test data and repos
if (Test-Path "crates/un-cli/tests/test_data") { Remove-Item -Recurse -Force "crates/un-cli/tests/test_data" }
if (Test-Path "crates/un-cli/tests/repos") { Remove-Item -Recurse -Force "crates/un-cli/tests/repos" }
if (Test-Path "tests/repos") { Remove-Item -Recurse -Force "tests/repos" }

# Remove cache and lock files
$unifiedDir = Join-Path $HOME ".unified"
if (Test-Path $unifiedDir) { Remove-Item -Recurse -Force $unifiedDir }
if (Test-Path "unified.lock") { Remove-Item -Force "unified.lock" }

Write-Host "Test environment cleanup complete!"
