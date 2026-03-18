#!/bin/bash
# Clean up test environment for unified repo manager

set -e

echo "Cleaning up test environment..."

# Remove test data and repos
rm -rf crates/un-cli/tests/test_data
rm -rf crates/un-cli/tests/repos
rm -rf tests/repos

# Remove cache and lock files
rm -rf ~/.unified
rm -f unified.lock

echo "Test environment cleanup complete!"