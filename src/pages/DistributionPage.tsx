import { useEffect, useMemo, useState } from "react";
import { BarChart } from "../components/BarChart";
import { useAppStore } from "../store/useAppStore";

function toPercent(x: number): string {
  return `${(x * 100).toFixed(2)}%`;
}

function toLocalInputValue(date: Date): string {
  if (isNaN(date.getTime())) return "";
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

export function DistributionPage() {
  const {
    statDefs,
    distribution,
    selectedStatKey,
    echoProbRows,
    distributionFilter,
    setDistributionFilter,
    setSelectedStatKey,
    refreshDistribution,
    refreshEchoProbRows,
  } = useAppStore();

  const [sortBy, setSortBy] = useState("pFinal");

  useEffect(() => {
    if (!selectedStatKey) {
      return;
    }
    void refreshEchoProbRows(sortBy);
  }, [selectedStatKey, sortBy, refreshEchoProbRows]);

  const chartData = useMemo(() => {
    const rows = distribution?.rows ?? [];
    return {
      labels: rows.map((row) => row.displayName),
      values: rows.map((row) => row.pGlobal),
    };
  }, [distribution]);

  return (
    <section className="page">
      <h2>实时概率分布</h2>
      <p className="hint">点击词条可查看包含该期望词条且当前仍可出的声骸列表。</p>

      <div className="card form-grid">
        <h3>筛选条件</h3>
        <label>
          开始时间
          <input
            type="datetime-local"
            value={distributionFilter.startTime ? toLocalInputValue(new Date(distributionFilter.startTime)) : ""}
            onChange={(e) =>
              setDistributionFilter({
                startTime: e.target.value ? new Date(e.target.value).toISOString() : undefined,
              })
            }
          />
        </label>
        <label>
          结束时间
          <input
            type="datetime-local"
            value={distributionFilter.endTime ? toLocalInputValue(new Date(distributionFilter.endTime)) : ""}
            onChange={(e) =>
              setDistributionFilter({
                endTime: e.target.value ? new Date(e.target.value).toISOString() : undefined,
              })
            }
          />
        </label>
        <label>
          主词条
          <select
            value={distributionFilter.mainStatKey ?? ""}
            onChange={(e) =>
              setDistributionFilter({ mainStatKey: e.target.value ? e.target.value : undefined })
            }
          >
            <option value="">全部</option>
            {statDefs.map((stat) => (
              <option key={stat.statKey} value={stat.statKey}>
                {stat.displayName}
              </option>
            ))}
          </select>
        </label>
        <label>
          Cost
          <select
            value={distributionFilter.costClass ?? ""}
            onChange={(e) =>
              setDistributionFilter({ costClass: e.target.value ? Number(e.target.value) : undefined })
            }
          >
            <option value="">全部</option>
            <option value="1">1</option>
            <option value="3">3</option>
            <option value="4">4</option>
          </select>
        </label>
        <label>
          状态
          <select
            value={distributionFilter.status ?? ""}
            onChange={(e) => setDistributionFilter({ status: e.target.value || undefined })}
          >
            <option value="">全部</option>
            <option value="tracking">tracking</option>
            <option value="paused">paused</option>
            <option value="abandoned">abandoned</option>
            <option value="completed">completed</option>
          </select>
        </label>
        <button type="button" onClick={() => void refreshDistribution()}>
          刷新分布
        </button>
      </div>

      <div className="card">
        <h3>全局概率图（总事件 {distribution?.totalEvents ?? 0}）</h3>
        <BarChart labels={chartData.labels} values={chartData.values} />
      </div>

      <div className="card">
        <h3>词条分布详情</h3>
        <table className="table">
          <thead>
            <tr>
              <th>词条</th>
              <th>计数</th>
              <th>P(global)</th>
              <th>Wilson CI</th>
              <th>Bayes Mean</th>
              <th>Bayes CI</th>
            </tr>
          </thead>
          <tbody>
            {(distribution?.rows ?? []).map((row) => (
              <tr
                key={row.statKey}
                className={row.statKey === selectedStatKey ? "active-row" : ""}
                onClick={() => {
                  setSelectedStatKey(row.statKey);
                  void refreshEchoProbRows(sortBy);
                }}
              >
                <td>{row.displayName}</td>
                <td>{row.count}</td>
                <td>{toPercent(row.pGlobal)}</td>
                <td>
                  {toPercent(row.ciFreqLow)} ~ {toPercent(row.ciFreqHigh)}
                </td>
                <td>{toPercent(row.bayesMean)}</td>
                <td>
                  {toPercent(row.bayesLow)} ~ {toPercent(row.bayesHigh)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <div className="card">
        <div className="inline-row">
          <h3>词条命中声骸列表 {selectedStatKey ? `(${selectedStatKey})` : ""}</h3>
          <label>
            排序
            <select value={sortBy} onChange={(e) => setSortBy(e.target.value)}>
              <option value="pFinal">P(final)</option>
              <option value="pNext">P(next)</option>
              <option value="rank">期望权重</option>
              <option value="slots">槽位</option>
            </select>
          </label>
        </div>

        <table className="table">
          <thead>
            <tr>
              <th>声骸</th>
              <th>槽位</th>
              <th>期望权重</th>
              <th>P(next)</th>
              <th>P(final)</th>
              <th>状态</th>
              <th>主词条</th>
              <th>Cost</th>
            </tr>
          </thead>
          <tbody>
            {echoProbRows.map((row) => (
              <tr key={row.echoId}>
                <td>{row.nickname ?? row.echoId.slice(0, 8)}</td>
                <td>{row.openedSlotsCount}/5</td>
                <td>{row.expectationRankMin}</td>
                <td>{toPercent(row.pNext)}</td>
                <td>{toPercent(row.pFinal)}</td>
                <td>{row.status}</td>
                <td>{row.mainStatKey}</td>
                <td>{row.costClass}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
