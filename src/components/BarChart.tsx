import { useEffect, useRef } from "react";
import * as echarts from "echarts";

interface BarChartProps {
  labels: string[];
  values: number[];
}

export function BarChart({ labels, values }: BarChartProps) {
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
  }, [labels, values]);

  return <div className="chart" ref={containerRef} />;
}
