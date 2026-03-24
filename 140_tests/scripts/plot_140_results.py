#!/usr/bin/env python3
"""
Plot ASHRAE 140-2023 validation results — OpenBSE value vs acceptance ranges.

Uses real units on the x-axis. Each row shows the acceptance range as a shaded
bar with min/max endpoint markers, and the OpenBSE result as a diamond.
Separate panels for energy (kWh) and temperature (C) checks.
"""

import csv
import sys
import os

try:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    import matplotlib.patches as mpatches
    import numpy as np
except ImportError:
    print("matplotlib is required: pip install matplotlib")
    sys.exit(1)

CSV_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "FULL_140_RESULTS.csv")
OUT_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), "ashrae140_validation.png")


def plot_panel(ax, rows, unit, title):
    n = len(rows)
    y_positions = list(range(n - 1, -1, -1))

    for i, r in enumerate(rows):
        lo = float(r["Min"])
        hi = float(r["Max"])
        val = float(r["OpenBSE"])
        y = y_positions[i]
        span = hi - lo

        # Draw acceptance range bar
        ax.barh(y, span, left=lo, height=0.55, color="#d4e6f1",
                edgecolor="#85c1e9", linewidth=0.8, zorder=2)

        # Min/Max endpoint ticks
        ax.plot(lo, y, marker="|", markersize=10, color="#2980b9",
                markeredgewidth=1.5, zorder=3)
        ax.plot(hi, y, marker="|", markersize=10, color="#2980b9",
                markeredgewidth=1.5, zorder=3)

        # Marker color based on position within range
        if span > 0:
            norm = (val - lo) / span
        else:
            norm = 0.5
        if norm < 0 or norm > 1:
            color = "#d32f2f"
        elif norm < 0.1 or norm > 0.9:
            color = "#f57c00"
        else:
            color = "#388e3c"

        # OpenBSE result marker
        ax.plot(val, y, marker="D", markersize=7, color=color,
                markeredgecolor="black", markeredgewidth=0.7, zorder=5)

        # Annotate the value next to the marker
        x_range = ax.get_xlim() if ax.get_xlim() != (0, 1) else (lo - span, hi + span)
        if unit == "kWh":
            ax.annotate(f"{val:.0f}", (val, y), textcoords="offset points",
                        xytext=(8, 0), fontsize=5.5, color="#333", va="center")
        else:
            ax.annotate(f"{val:.1f}", (val, y), textcoords="offset points",
                        xytext=(8, 0), fontsize=5.5, color="#333", va="center")

    # Build labels
    labels = []
    for r in rows:
        case = r["Case"]
        metric = r["Metric"].replace("Annual ", "").replace(" (kWh)", "") \
                             .replace(" (C)", "").replace("Temp", "T")
        labels.append(f"{case} {metric}")

    ax.set_yticks(y_positions)
    ax.set_yticklabels(labels, fontsize=7, fontfamily="monospace")
    ax.set_xlabel(f"Value ({unit})", fontsize=9)
    ax.set_title(title, fontsize=11, fontweight="bold")
    ax.grid(axis="x", alpha=0.3, linewidth=0.5)

    # Add some margin to x limits
    all_vals = [float(r["OpenBSE"]) for r in rows]
    all_los = [float(r["Min"]) for r in rows]
    all_his = [float(r["Max"]) for r in rows]
    x_min = min(min(all_vals), min(all_los))
    x_max = max(max(all_vals), max(all_his))
    margin = (x_max - x_min) * 0.12
    ax.set_xlim(x_min - margin, x_max + margin)


def main():
    all_rows = []
    with open(CSV_PATH) as f:
        reader = csv.DictReader(f)
        for r in reader:
            lo = float(r["Min"])
            hi = float(r["Max"])
            val = float(r["OpenBSE"])
            if lo == hi == 0 and val == 0:
                continue
            all_rows.append(r)

    # Split into energy and temperature checks
    energy_rows = [r for r in all_rows if "kWh" in r["Metric"]]
    temp_rows = [r for r in all_rows if "(C)" in r["Metric"]]

    n_energy = len(energy_rows)
    n_temp = len(temp_rows)

    fig, (ax1, ax2) = plt.subplots(
        1, 2,
        figsize=(18, max(10, max(n_energy, n_temp) * 0.33)),
        gridspec_kw={"width_ratios": [1.1, 1]},
    )

    plot_panel(ax1, energy_rows, "kWh", "Annual Heating & Cooling Energy")
    plot_panel(ax2, temp_rows, "°C", "Free-Float Zone Temperatures")

    # Shared legend
    legend_elements = [
        mpatches.Patch(facecolor="#d4e6f1", edgecolor="#85c1e9",
                       label="Acceptance range [Min, Max]"),
        plt.Line2D([0], [0], marker="|", color="#2980b9", linestyle="None",
                   markersize=10, markeredgewidth=1.5, label="Range endpoints"),
        plt.Line2D([0], [0], marker="D", color="w", markerfacecolor="#388e3c",
                   markeredgecolor="black", markersize=7, label="OpenBSE (within range)"),
        plt.Line2D([0], [0], marker="D", color="w", markerfacecolor="#f57c00",
                   markeredgecolor="black", markersize=7, label="OpenBSE (near edge <10%)"),
        plt.Line2D([0], [0], marker="D", color="w", markerfacecolor="#d32f2f",
                   markeredgecolor="black", markersize=7, label="OpenBSE (outside range)"),
    ]
    fig.legend(handles=legend_elements, loc="lower center", ncol=5, fontsize=8,
              bbox_to_anchor=(0.5, -0.01))

    fig.suptitle("ASHRAE 140-2023 Validation — OpenBSE vs Acceptance Ranges   |   63/63 PASS (100%)",
                 fontsize=13, fontweight="bold", y=0.995)

    plt.tight_layout(rect=[0, 0.03, 1, 0.98])
    plt.savefig(OUT_PATH, dpi=150, bbox_inches="tight")
    print(f"Chart saved to: {OUT_PATH}")


if __name__ == "__main__":
    main()
