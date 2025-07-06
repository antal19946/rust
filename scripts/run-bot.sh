#!/bin/bash

# Ultra-Low Latency Arbitrage Bot Runner
# This script sets up the optimal environment and runs the bot

set -e

echo "�� Starting Ultra-Low Latency Arbitrage Bot..."
echo "============================================="

# Check if we're on Linux
if [[ "$OSTYPE" != "linux-gnu"* ]]; then
    echo "❌ This bot is optimized for Linux. Please run on a Linux system."
    exit 1
fi

# Check Rust installation
if ! command -v cargo &> /dev/null; then
    echo "❌ Rust is not installed. Please install Rust first:"
    echo "   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
fi

# Check if .env file exists
if [ ! -f ".env" ]; then
    echo "❌ .env file not found. Please copy env.example to .env and configure it:"
    echo "   cp env.example .env"
    echo "   # Edit .env with your configuration"
    exit 1
fi

# Check HugePages availability
echo "�� Checking HugePages availability..."
HUGEPAGES_AVAILABLE=$(cat /proc/meminfo | grep -i hugepages_total | awk '{print $2}')
if [ "$HUGEPAGES_AVAILABLE" -gt 0 ]; then
    echo "✅ HugePages available: $HUGEPAGES_AVAILABLE pages"
else
    echo "⚠️  HugePages not available - bot will use regular memory"
fi

# Check available memory
echo "�� Checking available memory..."
TOTAL_MEM=$(free -g | awk 'NR==2{print $2}')
if [ "$TOTAL_MEM" -lt 16 ]; then
    echo "⚠️  Warning: Less than 16GB RAM available ($TOTAL_MEM GB)"
    echo "   For optimal performance, use 32GB+ RAM"
fi

# Build the bot
echo "�� Building arbitrage bot..."
cargo build --release

# Set performance optimizations
echo "⚡ Setting performance optimizations..."

# Set CPU governor to performance
if command -v cpupower &> /dev/null; then
    sudo cpupower frequency-set -g performance
    echo "✅ CPU governor set to performance"
fi

# Set process priority
echo "🎯 Setting process priority..."

# Run the bot with optimizations
echo "�� Starting arbitrage bot..."
echo "�� Monitor logs with: tail -f run.log"
echo "📈 Monitor opportunities with: tail -f arbitrage_opportunities.log"
echo "🛑 Press Ctrl+C to stop"

# Run with nice priority and real-time scheduling
sudo nice -n -20 ./target/release/arb-rust-bot 