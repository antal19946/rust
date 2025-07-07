#!/bin/bash

echo "🚀 Pair Filtering Script"
echo "========================"
echo ""
echo "Choose filtering method:"
echo "1. Simple filtering (fast, based on known tokens)"
echo "2. RPC-based filtering (slow, checks actual liquidity)"
echo "3. Both methods"
echo ""
read -p "Enter your choice (1-3): " choice

case $choice in
    1)
        echo "Running simple filtering..."
        cargo run --bin filter_liquid_pairs_simple
        ;;
    2)
        echo "Running RPC-based filtering..."
        echo "This will take a long time and make many RPC calls!"
        read -p "Are you sure? (y/N): " confirm
        if [[ $confirm == [yY] ]]; then
            cargo run --bin filter_liquid_pairs
        else
            echo "Cancelled."
            exit 0
        fi
        ;;
    3)
        echo "Running both filtering methods..."
        echo "Step 1: Simple filtering..."
        cargo run --bin filter_liquid_pairs_simple
        echo ""
        echo "Step 2: RPC-based filtering..."
        read -p "Continue with RPC filtering? (y/N): " confirm
        if [[ $confirm == [yY] ]]; then
            cargo run --bin filter_liquid_pairs
        fi
        ;;
    *)
        echo "Invalid choice. Exiting."
        exit 1
        ;;
esac

echo ""
echo "✅ Filtering completed!"
echo ""
echo "📁 Generated files:"
echo "   - data/liquid_pairs_v2.jsonl"
echo "   - data/liquid_pairs_v3.jsonl"
echo "   - data/liquid_pairs_combined.jsonl"
echo ""
echo "📊 Next steps:"
echo "   1. Update your bot to use liquid_pairs_combined.jsonl"
echo "   2. Test with the filtered pairs"
echo "   3. Adjust filtering criteria if needed" 