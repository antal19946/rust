#!/bin/bash

# Comprehensive Arbitrage Bot Test Script
# This script tests all components of the bot

set -e

echo "�� ARBITRAGE BOT COMPREHENSIVE TEST"
echo "==================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Test counters
TESTS_PASSED=0
TESTS_FAILED=0

# Function to print test results
print_test_result() {
    local test_name="$1"
    local result="$2"
    
    if [ "$result" = "PASS" ]; then
        echo -e "${GREEN}✅ $test_name: PASS${NC}"
        ((TESTS_PASSED++))
    else
        echo -e "${RED}❌ $test_name: FAIL${NC}"
        ((TESTS_FAILED++))
    fi
}

# Function to check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

echo "🔍 Testing environment..."

# Test 1: Check Rust installation
if command_exists cargo; then
    print_test_result "Rust Installation" "PASS"
else
    print_test_result "Rust Installation" "FAIL"
fi

# Test 2: Check if .env file exists
if [ -f ".env" ]; then
    print_test_result "Environment Configuration" "PASS"
else
    print_test_result "Environment Configuration" "FAIL"
    echo -e "${YELLOW}⚠️  Please copy env.example to .env and configure it${NC}"
fi

# Test 3: Check data files
echo "📁 Checking data files..."
if [ -f "data/pairs_v2.jsonl" ]; then
    print_test_result "V2 Pool Data" "PASS"
else
    print_test_result "V2 Pool Data" "FAIL"
fi

if [ -f "data/pairs_v3.jsonl" ]; then
    print_test_result "V3 Pool Data" "PASS"
else
    print_test_result "V3 Pool Data" "FAIL"
fi

if [ -f "data/safe_tokens.json" ]; then
    print_test_result "Safe Tokens Data" "PASS"
else
    print_test_result "Safe Tokens Data" "FAIL"
fi

# Test 4: Check HugePages
echo "�� Checking HugePages..."
HUGEPAGES_AVAILABLE=$(cat /proc/meminfo | grep -i hugepages_total | awk '{print $2}')
if [ "$HUGEPAGES_AVAILABLE" -gt 0 ]; then
    print_test_result "HugePages Support" "PASS"
    echo -e "${BLUE}   Available: $HUGEPAGES_AVAILABLE pages${NC}"
else
    print_test_result "HugePages Support" "FAIL"
    echo -e "${YELLOW}   HugePages not available - bot will use regular memory${NC}"
fi

# Test 5: Check memory
echo "🧠 Checking memory..."
TOTAL_MEM=$(free -g | awk 'NR==2{print $2}')
if [ "$TOTAL_MEM" -ge 16 ]; then
    print_test_result "Memory (16GB+)" "PASS"
    echo -e "${BLUE}   Available: $TOTAL_MEM GB${NC}"
else
    print_test_result "Memory (16GB+)" "FAIL"
    echo -e "${YELLOW}   Available: $TOTAL_MEM GB (16GB+ recommended)${NC}"
fi

# Test 6: Build the project
echo "🔨 Building project..."
if cargo build --release > build.log 2>&1; then
    print_test_result "Project Build" "PASS"
else
    print_test_result "Project Build" "FAIL"
    echo -e "${RED}Build failed. Check build.log for details.${NC}"
fi

# Test 7: Run critical function tests
echo "🧪 Running critical function tests..."
if timeout 30s cargo run --release --bin test-functions > test.log 2>&1; then
    print_test_result "Critical Functions" "PASS"
else
    print_test_result "Critical Functions" "FAIL"
    echo -e "${RED}Function tests failed. Check test.log for details.${NC}"
fi

# Test 8: Check if executable was created
if [ -f "target/release/arb-rust-bot" ]; then
    print_test_result "Executable Created" "PASS"
else
    print_test_result "Executable Created" "FAIL"
fi

# Test 9: Test configuration loading
echo "⚙️  Testing configuration..."
if timeout 10s cargo run --release --bin test-config > config.log 2>&1; then
    print_test_result "Configuration Loading" "PASS"
else
    print_test_result "Configuration Loading" "FAIL"
    echo -e "${RED}Configuration test failed. Check config.log for details.${NC}"
fi

# Test 10: Test cache initialization
echo "💾 Testing cache initialization..."
if timeout 15s cargo run --release --bin test-cache > cache.log 2>&1; then
    print_test_result "Cache Initialization" "PASS"
else
    print_test_result "Cache Initialization" "FAIL"
    echo -e "${RED}Cache test failed. Check cache.log for details.${NC}"
fi

# Print final results
echo ""
echo "📊 TEST RESULTS SUMMARY"
echo "======================="
echo -e "${GREEN}✅ Tests Passed: $TESTS_PASSED${NC}"
echo -e "${RED}❌ Tests Failed: $TESTS_FAILED${NC}"

if [ $TESTS_FAILED -eq 0 ]; then
    echo ""
    echo -e "${GREEN}🎉 ALL TESTS PASSED! Bot is ready to run.${NC}"
    echo ""
    echo "🚀 To start the bot:"
    echo "   cargo run --release"
    echo ""
    echo "📊 To monitor performance:"
    echo "   tail -f run.log"
    echo "   tail -f arbitrage_opportunities.log"
else
    echo ""
    echo -e "${RED}⚠️  Some tests failed. Please fix the issues before running the bot.${NC}"
    echo ""
    echo "�� Common fixes:"
    echo "   - Copy env.example to .env and configure it"
    echo "   - Ensure you have sufficient memory (16GB+)"
    echo "   - Check build.log, test.log, config.log, cache.log for details"
fi

echo ""
echo "🧪 Test completed at $(date)" 