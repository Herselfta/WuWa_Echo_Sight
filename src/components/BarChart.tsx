import { useEffect, useRef } from "react";
import * as echarts from "echarts";

interface BarChartProps {
  labels: string[];
  values: number[];
  tooltipRows?: Array<{
    count?: number;
    pGlobal?: number;
    ciFreqLow?: number;
    ciFreqHigh?: number;
    bayesMean?: number;
    bayesLow?: number;
    bayesHigh?: number;
  }>;
}

function toPercent(value: number | undefined): string {
  if (value === undefined || Number.isNaN(value)) return "-";
  return `${(value * 100).toFixed(2)}%`;
}

export function BarChart({ labels, values, tooltipRows }: BarChartProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!containerRef.current) {
      return;
    }

    const chart = echarts.init(containerRef.current);
    chart.setOption({
      animation: true,
      tooltip: {
        trigger: "axis",
        formatter: (params: unknown) => {
          const items = Array.isArray(params) ? params : [params];
          const point = items[0] as { dataIndex?: number; axisValueLabel?: string; data?: number } | undefined;
          const idx = point?.dataIndex ?? 0;
          const label = point?.axisValueLabel ?? labels[idx] ?? "";
          const row = tooltipRows?.[idx];
          if (row) {
            return [
              `<div><strong>${label}</strong></div>`,
              `<div>次数: ${row.count ?? "-"}</div>`,
              `<div>P(gbl): ${toPercent(row.pGlobal)}</div>`,
              `<div>Wilson CI: ${toPercent(row.ciFreqLow)} ~ ${toPercent(row.ciFreqHigh)}</div>`,
              `<div>Bayes: ${toPercent(row.bayesMean)} (${toPercent(row.bayesLow)}~${toPercent(row.bayesHigh)})</div>`,
            ].join("");
          }
          const value = typeof point?.data === "number" ? point.data : values[idx];
          return `<div><strong>${label}</strong></div><div>概率: ${toPercent(value)}</div>`;
        },
      },
      grid: {
        left: 40,
        right: 16,
        top: 20,
        bottom: 70,
      },
      xAxis: {
        type: "category",
        data: labels,
        axisLabel: {
          rotate: 45,
        },
      },
      yAxis: {
        type: "value",
        axisLabel: {
          formatter: (v: number) => `${(v * 100).toFixed(1)}%`,
        },
      },
      series: [
        {
          type: "bar",
          data: values,
          itemStyle: {
            color: "#0ea5e9",
          },
        },
      ],
    });

    const observer = new ResizeObserver(() => chart.resize());
    observer.observe(containerRef.current);

    return () => {
      observer.disconnect();
      chart.dispose();
    };
  }, [labels, values, tooltipRows]);

  return <div className="chart" ref={containerRef} />;
}
