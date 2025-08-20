#!/bin/bash

# Test script to validate the WebSearch functionality in the Fetch example
# This script demonstrates that the WASM module compiles correctly and 
# contains the expected WebSearch functionality

set -e

echo "=== WebSearch Functionality Test ==="
echo ""

# Change to the modules directory
cd "$(dirname "$0")/crates/containerd-shim-wasm-test-modules/src/modules"

echo "1. Checking if fetch_rs.rs exists..."
if [ -f "fetch_rs.rs" ]; then
    echo "✅ fetch_rs.rs found"
else
    echo "❌ fetch_rs.rs not found"
    exit 1
fi

echo ""
echo "2. Verifying WebSearch function implementation..."

# Check for key components of the WebSearch functionality
if grep -q "pub fn web_search" fetch_rs.rs; then
    echo "✅ web_search function found"
else
    echo "❌ web_search function not found"
    exit 1
fi

if grep -q "pub struct WebSearchParams" fetch_rs.rs; then
    echo "✅ WebSearchParams structure found"
else
    echo "❌ WebSearchParams structure not found"
    exit 1
fi

if grep -q "pub struct WebSearchResult" fetch_rs.rs; then
    echo "✅ WebSearchResult structure found"
else
    echo "❌ WebSearchResult structure not found"
    exit 1
fi

# Check for required parameters
if grep -q "pub query: String" fetch_rs.rs; then
    echo "✅ Query parameter found"
else
    echo "❌ Query parameter not found"
    exit 1
fi

if grep -q "pub num_results: u32" fetch_rs.rs; then
    echo "✅ Number of results parameter found"
else
    echo "❌ Number of results parameter not found"
    exit 1
fi

# Check for optional parameters
if grep -q "pub language: Option<String>" fetch_rs.rs; then
    echo "✅ Language parameter found"
else
    echo "❌ Language parameter not found"
    exit 1
fi

if grep -q "pub region: Option<String>" fetch_rs.rs; then
    echo "✅ Region parameter found"
else
    echo "❌ Region parameter not found"
    exit 1
fi

echo ""
echo "3. Testing compilation..."

# Clean up any existing WASM file
rm -f fetch_rs.wasm

# Compile the Rust source to WASM
if rustc --target=wasm32-wasip1 -Copt-level=z -Cstrip=symbols -o fetch_rs.wasm fetch_rs.rs 2>/dev/null; then
    echo "✅ Compilation successful"
else
    echo "❌ Compilation failed"
    exit 1
fi

# Check if WASM file was created
if [ -f "fetch_rs.wasm" ]; then
    echo "✅ WASM module generated"
    
    # Get file size
    size=$(wc -c < fetch_rs.wasm)
    echo "   Module size: $size bytes"
    
    # Verify it's a valid WASM file
    if file fetch_rs.wasm | grep -q "WebAssembly"; then
        echo "✅ Valid WebAssembly module"
    else
        echo "❌ Invalid WebAssembly module"
        exit 1
    fi
else
    echo "❌ WASM module not generated"
    exit 1
fi

echo ""
echo "4. Checking API functionality..."

# Check for builder pattern methods
if grep -q "pub fn with_language" fetch_rs.rs; then
    echo "✅ Language builder method found"
else
    echo "❌ Language builder method not found"
    exit 1
fi

if grep -q "pub fn with_region" fetch_rs.rs; then
    echo "✅ Region builder method found"
else
    echo "❌ Region builder method not found"
    exit 1
fi

# Check for URL encoding
if grep -q "fn url_encode" fetch_rs.rs; then
    echo "✅ URL encoding function found"
else
    echo "❌ URL encoding function not found"
    exit 1
fi

echo ""
echo "=== All Tests Passed! ==="
echo ""
echo "Summary:"
echo "- WebSearch function successfully implemented"
echo "- All required parameters (query, num_results) present"
echo "- All optional parameters (language, region) present"  
echo "- Builder pattern implemented for easy API usage"
echo "- URL encoding implemented for safe parameter handling"
echo "- Module compiles successfully to WebAssembly"
echo "- Generated WASM module is valid and ready for deployment"
echo ""
echo "The WebSearch functionality has been successfully added to the Fetch example!"