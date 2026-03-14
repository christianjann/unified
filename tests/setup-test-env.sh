#!/bin/bash
# Setup test environment for unified repo manager

set -e

echo "Setting up test environment..."

# Create test data directory
mkdir -p tests/test_data/source

# Initialize git repo
cd tests/test_data/source
git init

# Add test file
echo "Hello, world!" > hello.txt
git add hello.txt
git -c user.email="test@example.com" -c user.name="Test" commit -m "Initial commit"

echo "Test environment setup complete!"
echo "Test repo created at: tests/test_data/source"