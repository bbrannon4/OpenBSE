import { useMemo, useRef } from "react";
import ReactEChartsCore from "echarts-for-react/lib/core";
import * as echarts from "echarts/core";
import { LineChart } from "echarts/charts";
import {
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
  ToolboxComponent,
} from "echarts/components";
import { CanvasRenderer } from "echarts/renderers";
import type { ParsedCsv, AggregationMode } from "../lib/csv";
import { aggregateData } from "../lib/csv";

echarts.use([
  LineChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  DataZoomComponent,
  ToolboxComponent,
  CanvasRenderer,
]);

// Consistent colors for up to 20 series
const SERIES_COLORS = [
  "#7aa2f7", "#9ece6a", "#f7768e", "#e0af68", "#7dcfff",
  "#bb9af7", "#ff9e64", "#73daca", "#c0caf5", "#2ac3de",
  "#b4f9f8", "#ff007c", "#ffc777", "#c3e88d", "#89ddff",
  "#82aaff", "#ff5370", "#f78c6c", "#ffcb6b", "#c792ea",
];

interface TimeSeriesChartProps {
  parsed: ParsedCsv;
  selectedVarIndices: Set<number>;
  aggregation: AggregationMode;
}

export function TimeSeriesChart({
  parsed,
  selectedVarIndices,
  aggregation,
}: TimeSeriesChartProps) {
  const chartRef = useRef<ReactEChartsCore>(null);
  const indices = useMemo(
    () => Array.from(selectedVarIndices),
    [selectedVarIndices]
  );

  const { labels, series } = useMemo(
    () => aggregateData(parsed, indices, aggregation),
    [parsed, indices, aggregation]
  );

  const option = useMemo(() => {
    if (indices.length === 0) {
      return {
        graphic: {
          type: "text",
          left: "center",
          top: "center",
          style: {
            text: "Select variables from the left panel to chart",
            fill: "#565f89",
            fontSize: 14,
          },
        },
      };
    }

    // Group selected variables by unit for multi Y-axis
    const unitMap = new Map<string, number[]>();
    for (const idx of indices) {
      const v = parsed.variables[idx];
      const unit = v.unit === "-" ? "" : v.unit;
      const arr = unitMap.get(unit) ?? [];
      arr.push(idx);
      unitMap.set(unit, arr);
    }

    const unitList = Array.from(unitMap.keys());
    // Limit to 3 Y-axes max for readability
    const maxAxes = Math.min(unitList.length, 3);

    const yAxis = unitList.slice(0, maxAxes).map((unit, i) => ({
      type: "value" as const,
      name: unit || "Value",
      position: i === 0 ? ("left" as const) : ("right" as const),
      offset: i > 1 ? (i - 1) * 60 : 0,
      axisLabel: {
        color: "#9aa5ce",
        fontSize: 10,
      },
      axisLine: {
        show: true,
        lineStyle: { color: "#2f3146" },
      },
      splitLine: {
        lineStyle: { color: "#2f3146", type: "dashed" as const },
      },
      nameTextStyle: {
        color: "#9aa5ce",
        fontSize: 11,
      },
    }));

    // Map unit -> axis index (overflow goes to last axis)
    const unitToAxis = new Map<string, number>();
    for (let i = 0; i < unitList.length; i++) {
      unitToAxis.set(unitList[i], Math.min(i, maxAxes - 1));
    }

    let colorIdx = 0;
    const seriesConfig = indices.map((varIdx) => {
      const v = parsed.variables[varIdx];
      const unit = v.unit === "-" ? "" : v.unit;
      const axisIdx = unitToAxis.get(unit) ?? 0;
      const data = series.get(varIdx) ?? [];
      const color = SERIES_COLORS[colorIdx % SERIES_COLORS.length];
      colorIdx++;

      return {
        name: v.raw,
        type: "line" as const,
        yAxisIndex: axisIdx,
        data,
        showSymbol: false,
        lineStyle: { width: 1.5, color },
        itemStyle: { color },
        sampling: "lttb" as const,
        large: true,
        largeThreshold: 5000,
      };
    });

    return {
      backgroundColor: "transparent",
      animation: false,
      tooltip: {
        trigger: "axis" as const,
        backgroundColor: "#1f2033",
        borderColor: "#2f3146",
        textStyle: { color: "#c0caf5", fontSize: 11 },
        axisPointer: { type: "cross" as const },
        confine: true,
      },
      legend: {
        show: indices.length <= 10,
        type: "scroll" as const,
        bottom: 40,
        textStyle: { color: "#9aa5ce", fontSize: 10 },
        pageTextStyle: { color: "#9aa5ce" },
        pageIconColor: "#7aa2f7",
        pageIconInactiveColor: "#565f89",
        formatter: (name: string) => {
          // Shorten legend: show "Component:var" truncated
          return name.length > 40 ? name.slice(0, 37) + "..." : name;
        },
      },
      grid: {
        left: 60,
        right: maxAxes > 1 ? 60 + (maxAxes - 1) * 60 : 60,
        top: 20,
        bottom: indices.length <= 10 ? 100 : 60,
        containLabel: false,
      },
      xAxis: {
        type: "category" as const,
        data: labels,
        axisLabel: {
          color: "#9aa5ce",
          fontSize: 10,
          rotate: 0,
          interval: "auto" as const,
        },
        axisLine: { lineStyle: { color: "#2f3146" } },
      },
      yAxis,
      series: seriesConfig,
      dataZoom: [
        {
          type: "inside" as const,
          xAxisIndex: 0,
          filterMode: "none" as const,
        },
        {
          type: "slider" as const,
          xAxisIndex: 0,
          bottom: 5,
          height: 20,
          borderColor: "#2f3146",
          backgroundColor: "#1a1b26",
          fillerColor: "rgba(122, 162, 247, 0.15)",
          handleStyle: { color: "#7aa2f7" },
          textStyle: { color: "#9aa5ce", fontSize: 10 },
          dataBackground: {
            lineStyle: { color: "#3d59a1" },
            areaStyle: { color: "rgba(122, 162, 247, 0.1)" },
          },
        },
      ],
      toolbox: {
        right: 10,
        top: 0,
        feature: {
          dataZoom: {
            yAxisIndex: "none" as const,
            iconStyle: { borderColor: "#9aa5ce" },
          },
          restore: { iconStyle: { borderColor: "#9aa5ce" } },
        },
        iconStyle: { borderColor: "#565f89" },
        emphasis: { iconStyle: { borderColor: "#7aa2f7" } },
      },
    };
  }, [parsed, indices, labels, series]);

  return (
    <div className="chart-container">
      <ReactEChartsCore
        ref={chartRef}
        echarts={echarts}
        option={option}
        style={{ height: "100%", width: "100%" }}
        notMerge={true}
        lazyUpdate={true}
      />
    </div>
  );
}
