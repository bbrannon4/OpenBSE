#!/bin/bash
# Run all ASHRAE 140 600-series test cases

OPENBSE="/Users/benjaminbrannon/Documents/GitHub/New_EnergyPlus/target/release/openbse"
TESTS_DIR="/Users/benjaminbrannon/Documents/GitHub/OpenBSE-140_Tests"
RESULTS_DIR="$TESTS_DIR/results"

# Create results directory
mkdir -p "$RESULTS_DIR"

# List of all 600-series cases
CASES="600 600ff 620 640 650 650ff 660 670 680 680ff 685 695"

echo "=========================================="
echo " ASHRAE 140 600-Series Test Runner"
echo "=========================================="
echo ""

for case in $CASES; do
    echo "Running Case $case..."
    cd "$TESTS_DIR"
    
    # Create case-specific results subdirectory
    CASE_DIR="$RESULTS_DIR/case$case"
    mkdir -p "$CASE_DIR"
    
    # Run simulation
    $OPENBSE "ashrae140_case${case}.yaml" -o "$CASE_DIR/results.csv" 2>&1 | tee "$CASE_DIR/run.log"
    
    # Move output files to case directory
    [ -f zone_results.csv ] && mv zone_results.csv "$CASE_DIR/"
    [ -f energy_monthly.csv ] && mv energy_monthly.csv "$CASE_DIR/"
    [ -f summary_report.txt ] && mv summary_report.txt "$CASE_DIR/"
    
    echo "  ✓ Results saved to $CASE_DIR"
    echo ""
done

echo "=========================================="
echo " All tests complete!"
echo "=========================================="
echo "Results saved to: $RESULTS_DIR"
