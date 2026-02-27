import { useEffect, useMemo, useState } from "react";
import { appendOrderedEvent, getEventHistory } from "../api/tauri";
import { useAppStore } from "../store/useAppStore";
import type { EventRow } from "../types/domain";

function toLocalInputValue(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

function normalizeLocalTime(value: string): string {
  const dt = new Date(value);
  return dt.toISOString();
}

function formatScaledValue(unit: string, valueScaled: number) {
  return unit === "percent" ? `${(valueScaled / 10).toFixed(1)}%` : String(valueScaled);
}

export function RecordPage() {
  const { echoes, statDefs, refreshEchoes } = useAppStore();
  const [selectedEchoId, setSelectedEchoId] = useState<string>("");
  const [slotNo, setSlotNo] = useState<number>(1);
  const [statKey, setStatKey] = useState<string>("crit_rate");
  const [tierIndex, setTierIndex] = useState<number>(1);
  const [eventTimeLocal, setEventTimeLocal] = useState<string>(toLocalInputValue(new Date()));
  const [eventHistory, setEventHistory] = useState<EventRow[]>([]);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState("");

  const selectedEcho = useMemo(
    () => echoes.find((echo) => echo.echoId === selectedEchoId) ?? null,
    [echoes, selectedEchoId],
  );

  const selectedStat = useMemo(
    () => statDefs.find((stat) => stat.statKey === statKey) ?? null,
    [statDefs, statKey],
  );

  useEffect(() => {
    if (!selectedEcho && echoes.length > 0) {
      const defaultEcho = echoes[0];
      setSelectedEchoId(defaultEcho.echoId);
      setSlotNo(Math.min(defaultEcho.openedSlotsCount + 1, 5));
    }
  }, [echoes, selectedEcho]);

  useEffect(() => {
    if (!selectedEcho) {
      return;
    }
    setSlotNo(Math.min(selectedEcho.openedSlotsCount + 1, 5));
  }, [selectedEcho]);

  useEffect(() => {
    if (!selectedStat) {
      return;
    }
    setTierIndex(selectedStat.tiers[0]?.tierIndex ?? 1);
  }, [selectedStat]);

  const loadHistory = async () => {
    const rows = await getEventHistory({ limit: 200 });
    setEventHistory(rows);
  };

  useEffect(() => {
    void loadHistory();
  }, []);

  const handleSubmit = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setMessage("");

    try {
      const result = await appendOrderedEvent({
        echoId: selectedEchoId,
        slotNo,
        statKey,
        tierIndex,
        eventTime: normalizeLocalTime(eventTimeLocal),
      });

      await Promise.all([refreshEchoes(), loadHistory()]);
      setMessage(`录入成功，eventId: ${result.eventId}`);
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const selectedTierValue = selectedStat?.tiers.find((x) => x.tierIndex === tierIndex)?.valueScaled ?? 0;

  return (
    <section className="page">
      <h2>快速录入</h2>
      <p className="hint">只录入有明确顺序的强化事件，自动进入全局统计流。</p>

      <form className="card form-grid" onSubmit={handleSubmit}>
        <label>
          声骸
          <select value={selectedEchoId} onChange={(e) => setSelectedEchoId(e.target.value)}>
            {echoes.map((echo) => (
              <option key={echo.echoId} value={echo.echoId}>
                {(echo.nickname ?? echo.echoId.slice(0, 8)) + ` (${echo.openedSlotsCount}/5)`}
              </option>
            ))}
          </select>
        </label>

        <label>
          槽位
          <select value={slotNo} onChange={(e) => setSlotNo(Number(e.target.value))}>
            {[1, 2, 3, 4, 5].map((x) => (
              <option key={x} value={x}>
                {x}
              </option>
            ))}
          </select>
        </label>

        <label>
          词条
          <select value={statKey} onChange={(e) => setStatKey(e.target.value)}>
            {statDefs.map((stat) => (
              <option key={stat.statKey} value={stat.statKey}>
                {stat.displayName}
              </option>
            ))}
          </select>
        </label>

        <label>
          档位
          <select value={tierIndex} onChange={(e) => setTierIndex(Number(e.target.value))}>
            {(selectedStat?.tiers ?? []).map((tier) => (
              <option key={tier.tierIndex} value={tier.tierIndex}>
                档位 {tier.tierIndex} ({formatScaledValue(selectedStat?.unit ?? "flat", tier.valueScaled)})
              </option>
            ))}
          </select>
        </label>

        <label>
          事件时间
          <input
            type="datetime-local"
            value={eventTimeLocal}
            onChange={(e) => setEventTimeLocal(e.target.value)}
          />
        </label>

        <div className="inline-row">
          <span>当前值预览：{formatScaledValue(selectedStat?.unit ?? "flat", selectedTierValue)}</span>
          <button type="submit" disabled={saving || !selectedEchoId}>
            保存事件
          </button>
        </div>
      </form>

      {message ? <p className="message">{message}</p> : null}

      <div className="card">
        <h3>最近事件</h3>
        <table className="table">
          <thead>
            <tr>
              <th>analysisSeq</th>
              <th>声骸</th>
              <th>槽位</th>
              <th>词条</th>
              <th>档位</th>
              <th>值</th>
              <th>事件时间</th>
            </tr>
          </thead>
          <tbody>
            {eventHistory.map((row) => {
              const stat = statDefs.find((s) => s.statKey === row.statKey);
              return (
                <tr key={row.eventId}>
                  <td>{row.analysisSeq}</td>
                  <td>{row.echoNickname ?? row.echoId.slice(0, 8)}</td>
                  <td>{row.slotNo}</td>
                  <td>{stat?.displayName ?? row.statKey}</td>
                  <td>{row.tierIndex}</td>
                  <td>{formatScaledValue(stat?.unit ?? "flat", row.valueScaled)}</td>
                  <td>{new Date(row.eventTime).toLocaleString()}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </section>
  );
}
