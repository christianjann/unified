# Setup test environment for unified repo manager

$ErrorActionPreference = "Stop"

Write-Host "Setting up test environment..."

# Create test data directory
New-Item -ItemType Directory -Force -Path "tests/test_data/source" | Out-Null

# Initialize git repo
Push-Location "tests/test_data/source"
git init

# Add test file
Set-Content -Path "hello.txt" -Value "Hello, world!"
git add hello.txt
git -c user.email="test@example.com" -c user.name="Test" commit -m "Initial commit"
Pop-Location

Write-Host "Test environment setup complete!"
Write-Host "Test repo created at: tests/test_data/source"
