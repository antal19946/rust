#!/bin/bash

echo "🧪 QUICK ARBITRAGE BOT TEST"
echo "============================"

# Check if .env exists
if [ ! -f ".env" ]; then
    echo "❌ .env file not found. Creating from example..."
    cp env.example .env
    echo "⚠️  Please edit .env with your configuration before running tests"
    exit 1
fi

# Build the project
echo "🔨 Building project..."
cargo build --release

# Test 1: Configuration
echo ""
echo "🧪 Test 1: Configuration Loading"
echo "--------------------------------"
./target/release/test-config

# Test 2: Cache
echo ""
echo "🧪 Test 2: Cache Initialization"
echo "-------------------------------"
./target/release/test-cache

# Test 3: Critical Functions
echo ""
echo "🧪 Test 3: Critical Functions"
echo "----------------------------"
./target/release/test-functions

# Test 4: Main Bot (short run)
echo ""
echo "�� Test 4: Main Bot (5 second test)"
echo "-----------------------------------"
timeout 5s ./target/release/arb-rust-bot || echo "Bot stopped after 5 seconds (expected)"

echo ""
echo "✅ Quick test completed!"
echo "📊 Check the logs above for any errors" 