import { useEffect, useState } from "react";
import {
  createProbabilitySnapshot,
  editOrderedEvent,
  exportCsv,
  getEventHistory,
} from "../api/tauri";
import { useAppStore } from "../store/useAppStore";
import type { EventRow } from "../types/domain";

function toLocalInputValue(iso: string): string {
  const date = new Date(iso);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

export function AnalysisPage() {
  const { distributionFilter, selectedStatKey } = useAppStore();
  const [events, setEvents] = useState<EventRow[]>([]);
  const [eventId, setEventId] = useState("");
  const [slotNo, setSlotNo] = useState("");
  const [statKey, setStatKey] = useState("");
  const [tierIndex, setTierIndex] = useState("");
  const [eventTime, setEventTime] = useState("");
  const [reorderMode, setReorderMode] = useState<"none" | "time_assist">("none");
  const [message, setMessage] = useState("");
  const [loading, setLoading] = useState(false);

  const loadEvents = async () => {
    const rows = await getEventHistory({ limit: 200 });
    setEvents(rows);
  };

  useEffect(() => {
    void loadEvents();
  }, []);

  const fillFromRow = (row: EventRow) => {
    setEventId(row.eventId);
    setSlotNo(String(row.slotNo));
    setStatKey(row.statKey);
    setTierIndex(String(row.tierIndex));
    setEventTime(toLocalInputValue(row.eventTime));
  };

  const submitEdit = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!eventId) {
      setMessage("请先输入 eventId。");
      return;
    }
    setLoading(true);
    setMessage("");
    try {
      const result = await editOrderedEvent({
        eventId,
        slotNo: slotNo ? Number(slotNo) : undefined,
        statKey: statKey || undefined,
        tierIndex: tierIndex ? Number(tierIndex) : undefined,
        eventTime: eventTime ? new Date(eventTime).toISOString() : undefined,
        reorderMode,
      });
      await loadEvents();
      setMessage(`事件修正成功，影响范围: ${result.affectedRange}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  };

  const snapshot = async () => {
    setLoading(true);
    setMessage("");
    try {
      const result = await createProbabilitySnapshot({
        scope: distributionFilter,
        statKey: selectedStatKey ?? undefined,
      });
      setMessage(`快照创建成功: ${result.snapshotId}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  };

  const doExport = async () => {
    setLoading(true);
    setMessage("");
    try {
      const result = await exportCsv({
        scope: distributionFilter,
        includeSnapshots: true,
      });
      setMessage(`CSV 已导出: ${result.zipPath}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setLoading(false);
    }
  };

  return (
    <section className="page">
      <h2>分析与修正</h2>
      <p className="hint">事件修正需二次确认，你可以选择是否执行 time_assist 重排。</p>

      <div className="card inline-row">
        <button type="button" onClick={() => void snapshot()} disabled={loading}>
          生成概率快照
        </button>
        <button type="button" onClick={() => void doExport()} disabled={loading}>
          导出 CSV(zip)
        </button>
      </div>

      <form className="card form-grid" onSubmit={submitEdit}>
        <h3>事件修正</h3>
        <label>
          Event ID
          <input value={eventId} onChange={(e) => setEventId(e.target.value)} placeholder="必填" />
        </label>
        <label>
          槽位
          <input value={slotNo} onChange={(e) => setSlotNo(e.target.value)} placeholder="可选" />
        </label>
        <label>
          词条 key
          <input value={statKey} onChange={(e) => setStatKey(e.target.value)} placeholder="可选" />
        </label>
        <label>
          档位
          <input value={tierIndex} onChange={(e) => setTierIndex(e.target.value)} placeholder="可选" />
        </label>
        <label>
          时间
          <input type="datetime-local" value={eventTime} onChange={(e) => setEventTime(e.target.value)} />
        </label>
        <label>
          重排模式
          <select
            value={reorderMode}
            onChange={(e) => setReorderMode(e.target.value as "none" | "time_assist")}
          >
            <option value="none">none</option>
            <option value="time_assist">time_assist</option>
          </select>
        </label>
        <button
          type="submit"
          disabled={loading}
          onClick={(e) => {
            if (!window.confirm("确认修改事件？修改后将重算相关统计。")) {
              e.preventDefault();
            }
          }}
        >
          提交修正
        </button>
      </form>

      {message ? <p className="message">{message}</p> : null}

      <div className="card">
        <h3>最近 200 条事件（点击填充修正表单）</h3>
        <table className="table">
          <thead>
            <tr>
              <th>analysisSeq</th>
              <th>eventId</th>
              <th>声骸</th>
              <th>槽位</th>
              <th>词条</th>
              <th>档位</th>
              <th>时间</th>
              <th>动作</th>
            </tr>
          </thead>
          <tbody>
            {events.map((row) => (
              <tr key={row.eventId}>
                <td>{row.analysisSeq}</td>
                <td className="mono-cell">{row.eventId}</td>
                <td>{row.echoNickname ?? row.echoId.slice(0, 8)}</td>
                <td>{row.slotNo}</td>
                <td>{row.statKey}</td>
                <td>{row.tierIndex}</td>
                <td>{new Date(row.eventTime).toLocaleString()}</td>
                <td>
                  <button type="button" onClick={() => fillFromRow(row)}>
                    填充
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
