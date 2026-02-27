import { useEffect, useMemo, useState } from "react";
import { createEcho, setExpectations, updateEcho, upsertBackfillState } from "../api/tauri";
import { useAppStore } from "../store/useAppStore";
import type { EchoStatus } from "../types/domain";

interface ExpectationDraft {
  statKey: string;
  rank: number;
}

interface BackfillDraft {
  slotNo: number;
  statKey: string;
  tierIndex: number;
}

function formatScaledValue(unit: string, valueScaled: number) {
  if (unit === "percent") {
    return `${(valueScaled / 10).toFixed(1)}%`;
  }
  return String(valueScaled);
}

export function EchoPoolPage() {
  const { statDefs, echoes, refreshEchoes } = useAppStore();
  const [selectedEchoId, setSelectedEchoId] = useState<string | null>(null);
  const [createForm, setCreateForm] = useState({
    nickname: "",
    mainStatKey: "atk_pct",
    costClass: 1,
    status: "tracking" as EchoStatus,
  });
  const [expectationDrafts, setExpectationDrafts] = useState<ExpectationDraft[]>([]);
  const [backfillDrafts, setBackfillDrafts] = useState<BackfillDraft[]>([]);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<string>("");

  const statMap = useMemo(() => {
    return new Map(statDefs.map((s) => [s.statKey, s]));
  }, [statDefs]);

  const selectedEcho = useMemo(
    () => echoes.find((echo) => echo.echoId === selectedEchoId) ?? null,
    [echoes, selectedEchoId],
  );

  useEffect(() => {
    if (!selectedEcho) {
      return;
    }

    setExpectationDrafts(
      selectedEcho.expectations.map((x) => ({
        statKey: x.statKey,
        rank: x.rank,
      })),
    );

    setBackfillDrafts(
      selectedEcho.currentSubstats
        .filter((slot) => slot.source === "backfill")
        .map((slot) => ({
          slotNo: slot.slotNo,
          statKey: slot.statKey,
          tierIndex: slot.tierIndex,
        })),
    );
  }, [selectedEcho]);

  const handleCreate = async (event: React.FormEvent) => {
    event.preventDefault();
    setSaving(true);
    setMessage("");
    try {
      await createEcho({
        nickname: createForm.nickname || undefined,
        mainStatKey: createForm.mainStatKey,
        costClass: createForm.costClass,
        status: createForm.status,
      });
      await refreshEchoes();
      setCreateForm((prev) => ({ ...prev, nickname: "" }));
      setMessage("声骸创建成功。");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const handleStatusChange = async (echoId: string, status: EchoStatus) => {
    try {
      await updateEcho({ echoId, status });
      await refreshEchoes();
    } catch (error) {
      setMessage(String(error));
    }
  };

  const saveExpectations = async () => {
    if (!selectedEcho) {
      return;
    }

    setSaving(true);
    setMessage("");
    try {
      await setExpectations(
        selectedEcho.echoId,
        expectationDrafts
          .filter((x) => x.statKey)
          .map((x) => ({
            statKey: x.statKey,
            rank: x.rank,
          })),
      );
      await refreshEchoes();
      setMessage("期望词条已保存。");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  const saveBackfill = async () => {
    if (!selectedEcho) {
      return;
    }
    setSaving(true);
    setMessage("");
    try {
      await upsertBackfillState({
        echoId: selectedEcho.echoId,
        slots: backfillDrafts
          .filter((x) => x.statKey)
          .map((x) => ({
            slotNo: x.slotNo,
            statKey: x.statKey,
            tierIndex: x.tierIndex,
          })),
      });
      await refreshEchoes();
      setMessage("补录状态已保存（不进入统计流）。");
    } catch (error) {
      setMessage(String(error));
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="page">
      <h2>声骸池</h2>
      <p className="hint">管理声骸档案、期望词条和当前补录状态。</p>

      <form className="card form-grid" onSubmit={handleCreate}>
        <h3>新建声骸</h3>
        <label>
          昵称
          <input
            value={createForm.nickname}
            onChange={(e) => setCreateForm((prev) => ({ ...prev, nickname: e.target.value }))}
            placeholder="可选"
          />
        </label>

        <label>
          主词条
          <select
            value={createForm.mainStatKey}
            onChange={(e) => setCreateForm((prev) => ({ ...prev, mainStatKey: e.target.value }))}
          >
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
            value={createForm.costClass}
            onChange={(e) => setCreateForm((prev) => ({ ...prev, costClass: Number(e.target.value) }))}
          >
            <option value={1}>1</option>
            <option value={3}>3</option>
            <option value={4}>4</option>
          </select>
        </label>

        <label>
          状态
          <select
            value={createForm.status}
            onChange={(e) => setCreateForm((prev) => ({ ...prev, status: e.target.value as EchoStatus }))}
          >
            <option value="tracking">tracking</option>
            <option value="paused">paused</option>
            <option value="abandoned">abandoned</option>
            <option value="completed">completed</option>
          </select>
        </label>

        <button type="submit" disabled={saving}>
          创建
        </button>
      </form>

      {message ? <p className="message">{message}</p> : null}

      <div className="card">
        <h3>声骸列表</h3>
        <table className="table">
          <thead>
            <tr>
              <th>昵称</th>
              <th>主词条</th>
              <th>Cost</th>
              <th>槽位</th>
              <th>状态</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {echoes.map((echo) => (
              <tr key={echo.echoId} className={echo.echoId === selectedEchoId ? "active-row" : ""}>
                <td>{echo.nickname ?? echo.echoId.slice(0, 8)}</td>
                <td>{echo.mainStatKey}</td>
                <td>{echo.costClass}</td>
                <td>{echo.openedSlotsCount}/5</td>
                <td>
                  <select
                    value={echo.status}
                    onChange={(e) => {
                      void handleStatusChange(echo.echoId, e.target.value as EchoStatus);
                    }}
                  >
                    <option value="tracking">tracking</option>
                    <option value="paused">paused</option>
                    <option value="abandoned">abandoned</option>
                    <option value="completed">completed</option>
                  </select>
                </td>
                <td>
                  <button type="button" onClick={() => setSelectedEchoId(echo.echoId)}>
                    详情
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      {selectedEcho ? (
        <div className="card split-card">
          <div>
            <h3>期望词条（{selectedEcho.nickname ?? selectedEcho.echoId.slice(0, 8)}）</h3>
            {expectationDrafts.map((item, idx) => (
              <div className="inline-row" key={`exp-${idx}`}>
                <select
                  value={item.statKey}
                  onChange={(e) => {
                    const value = e.target.value;
                    setExpectationDrafts((prev) =>
                      prev.map((row, rowIdx) => (rowIdx === idx ? { ...row, statKey: value } : row)),
                    );
                  }}
                >
                  {statDefs.map((stat) => (
                    <option key={stat.statKey} value={stat.statKey}>
                      {stat.displayName}
                    </option>
                  ))}
                </select>
                <input
                  type="number"
                  min={1}
                  value={item.rank}
                  onChange={(e) => {
                    const value = Number(e.target.value);
                    setExpectationDrafts((prev) =>
                      prev.map((row, rowIdx) => (rowIdx === idx ? { ...row, rank: value } : row)),
                    );
                  }}
                />
                <button
                  type="button"
                  onClick={() => setExpectationDrafts((prev) => prev.filter((_, rowIdx) => rowIdx !== idx))}
                >
                  删除
                </button>
              </div>
            ))}
            <div className="inline-row">
              <button
                type="button"
                onClick={() =>
                  setExpectationDrafts((prev) => [
                    ...prev,
                    {
                      statKey: statDefs[0]?.statKey ?? "crit_rate",
                      rank: 1,
                    },
                  ])
                }
              >
                添加期望词条
              </button>
              <button type="button" onClick={() => void saveExpectations()} disabled={saving}>
                保存期望
              </button>
            </div>
          </div>

          <div>
            <h3>补录当前状态（无序，不进统计）</h3>
            {backfillDrafts.map((slot, idx) => {
              const stat = statMap.get(slot.statKey);
              const tiers = stat?.tiers ?? [];
              const valueScaled = tiers.find((t) => t.tierIndex === slot.tierIndex)?.valueScaled ?? 0;

              return (
                <div className="inline-row" key={`backfill-${idx}`}>
                  <select
                    value={slot.slotNo}
                    onChange={(e) => {
                      const value = Number(e.target.value);
                      setBackfillDrafts((prev) =>
                        prev.map((row, rowIdx) => (rowIdx === idx ? { ...row, slotNo: value } : row)),
                      );
                    }}
                  >
                    {[1, 2, 3, 4, 5].map((slotNo) => (
                      <option key={slotNo} value={slotNo}>
                        槽位 {slotNo}
                      </option>
                    ))}
                  </select>
                  <select
                    value={slot.statKey}
                    onChange={(e) => {
                      const nextStatKey = e.target.value;
                      const nextTiers = statMap.get(nextStatKey)?.tiers ?? [];
                      setBackfillDrafts((prev) =>
                        prev.map((row, rowIdx) =>
                          rowIdx === idx
                            ? {
                                ...row,
                                statKey: nextStatKey,
                                tierIndex: nextTiers[0]?.tierIndex ?? 1,
                              }
                            : row,
                        ),
                      );
                    }}
                  >
                    {statDefs.map((item) => (
                      <option key={item.statKey} value={item.statKey}>
                        {item.displayName}
                      </option>
                    ))}
                  </select>
                  <select
                    value={slot.tierIndex}
                    onChange={(e) => {
                      const value = Number(e.target.value);
                      setBackfillDrafts((prev) =>
                        prev.map((row, rowIdx) => (rowIdx === idx ? { ...row, tierIndex: value } : row)),
                      );
                    }}
                  >
                    {tiers.map((tier) => (
                      <option key={tier.tierIndex} value={tier.tierIndex}>
                        档位 {tier.tierIndex} ({formatScaledValue(stat?.unit ?? "flat", tier.valueScaled)})
                      </option>
                    ))}
                  </select>
                  <span className="value-preview">{formatScaledValue(stat?.unit ?? "flat", valueScaled)}</span>
                  <button
                    type="button"
                    onClick={() => setBackfillDrafts((prev) => prev.filter((_, rowIdx) => rowIdx !== idx))}
                  >
                    删除
                  </button>
                </div>
              );
            })}

            <div className="inline-row">
              <button
                type="button"
                onClick={() =>
                  setBackfillDrafts((prev) => [
                    ...prev,
                    {
                      slotNo: 1,
                      statKey: statDefs[0]?.statKey ?? "crit_rate",
                      tierIndex: 1,
                    },
                  ])
                }
              >
                添加补录槽位
              </button>
              <button type="button" onClick={() => void saveBackfill()} disabled={saving}>
                保存补录
              </button>
            </div>
          </div>

          <div>
            <h3>当前副词条状态</h3>
            <ul className="slot-list">
              {selectedEcho.currentSubstats.map((slot) => {
                const stat = statMap.get(slot.statKey);
                return (
                  <li key={`${slot.slotNo}-${slot.statKey}`}>
                    槽位 {slot.slotNo}: {stat?.displayName ?? slot.statKey} / 档位 {slot.tierIndex} /
                    {" "}
                    {formatScaledValue(stat?.unit ?? "flat", slot.valueScaled)} / 来源 {slot.source}
                  </li>
                );
              })}
            </ul>
          </div>
        </div>
      ) : null}
    </section>
  );
}
