import { useState } from 'react';
import { useStore } from '../store';

export default function ExperimentSidebar() {
  const ir = useStore((s) => s.ir);
  const scenarios = useStore((s) => s.scenarios);
  const runConfig = useStore((s) => s.runConfig);
  const addScenario = useStore((s) => s.addScenario);
  const removeScenario = useStore((s) => s.removeScenario);
  const renameScenario = useStore((s) => s.renameScenario);
  const setScenarioParam = useStore((s) => s.setScenarioParam);
  const clearScenarioParam = useStore((s) => s.clearScenarioParam);

  const [selectedId, setSelectedId] = useState<string>(() => scenarios[0]?.id ?? '');
  const [editingName, setEditingName] = useState<string | null>(null);

  const selected = scenarios.find((s) => s.id === selectedId) ?? scenarios[0];
  const isBaseline = selected?.id === scenarios[0]?.id;

  const irParams = ir?.parameters ?? [];

  return (
    <div className="flex flex-col h-full overflow-hidden bg-surface-1 border-r border-surface-border">

      {/* ── Scenarios ─────────────────────────────────────────────────────────── */}
      <div className="px-3 pt-3 pb-2 border-b border-surface-border flex-shrink-0">
        <div className="text-xs text-gray-500 font-medium mb-2 uppercase tracking-wider">Scenarios</div>
        <div className="flex flex-wrap gap-1.5 mb-2">
          {scenarios.map((sc, idx) => (
            <button
              key={sc.id}
              onClick={() => setSelectedId(sc.id)}
              className={`flex items-center gap-1.5 px-2 py-1 rounded text-xs transition-colors ${
                sc.id === selected?.id
                  ? 'ring-1 ring-offset-0 bg-surface-3 text-gray-100'
                  : 'bg-surface-2 text-gray-400 hover:text-gray-200'
              }`}
              style={{ borderLeft: `3px solid ${sc.color}` }}
            >
              {sc.status === 'running' && <span className="animate-pulse text-accent">●</span>}
              {sc.status === 'ok' && sc.runs.length > 0 && <span style={{ color: sc.color }}>●</span>}
              {sc.status === 'error' && <span className="text-red-400">●</span>}
              {sc.status === 'idle' && <span className="text-gray-600">○</span>}
              <span>{idx === 0 ? 'Baseline' : sc.name}</span>
              {idx > 0 && (
                <span
                  className="text-gray-600 hover:text-red-400 ml-0.5 leading-none"
                  onClick={(e) => {
                    e.stopPropagation();
                    removeScenario(sc.id);
                    if (selectedId === sc.id) setSelectedId(scenarios[0]?.id ?? '');
                  }}
                >
                  ×
                </span>
              )}
            </button>
          ))}
        </div>

        <div className="flex gap-1.5">
          <button
            onClick={() => addScenario(false)}
            disabled={!ir}
            className="px-2 py-0.5 text-xs text-gray-400 hover:text-gray-200 border border-surface-border rounded transition-colors disabled:opacity-40"
          >
            + Clone last
          </button>
          <button
            onClick={() => addScenario(true)}
            disabled={!ir}
            className="px-2 py-0.5 text-xs text-gray-400 hover:text-gray-200 border border-surface-border rounded transition-colors disabled:opacity-40"
          >
            + From baseline
          </button>
        </div>
      </div>

      {/* ── Selected scenario params ───────────────────────────────────────────── */}
      {selected && (
        <div className="flex-1 overflow-y-auto px-3 py-2 min-h-0">
          {/* Scenario name */}
          <div className="flex items-center gap-2 mb-2">
            <div className="w-2.5 h-2.5 rounded-full flex-shrink-0" style={{ backgroundColor: selected.color }} />
            {!isBaseline && editingName === selected.id ? (
              <input
                className="flex-1 text-xs bg-surface-2 border border-accent rounded px-1.5 py-0.5 text-gray-100 focus:outline-none"
                autoFocus
                value={selected.name}
                onChange={(e) => renameScenario(selected.id, e.target.value)}
                onBlur={() => setEditingName(null)}
                onKeyDown={(e) => { if (e.key === 'Enter' || e.key === 'Escape') setEditingName(null); }}
              />
            ) : (
              <span
                className={`text-xs text-gray-300 ${!isBaseline ? 'cursor-pointer hover:text-gray-100' : ''}`}
                onClick={() => { if (!isBaseline) setEditingName(selected.id); }}
              >
                {isBaseline ? 'Baseline' : selected.name}
              </span>
            )}
            {selected.seedsCompleted > 0 && (
              <span className="text-xs text-gray-600 ml-auto">
                {selected.seedsCompleted}/{runConfig.nSeeds}
              </span>
            )}
          </div>

          {/* Progress bar */}
          {selected.seedsCompleted > 0 && (
            <div className="w-full h-0.5 bg-surface-2 rounded mb-2">
              <div
                className="h-full rounded transition-all"
                style={{
                  width: `${(selected.seedsCompleted / runConfig.nSeeds) * 100}%`,
                  backgroundColor: selected.color,
                }}
              />
            </div>
          )}

          {/* Param overrides */}
          {irParams.length > 0 && (
            <>
              {!isBaseline && Object.keys(selected.paramOverrides).length === 0 && (
                <div className="text-xs text-gray-600 italic mb-1">
                  No overrides — runs with baseline values.
                </div>
              )}
              <div className="flex flex-col gap-1">
                {irParams.map((p) => {
                  const overridden = selected.paramOverrides[p.name] !== undefined;
                  const irValue = (p.value as number | null);
                  const baselineVal = isBaseline
                    ? null
                    : (scenarios[0]?.paramOverrides[p.name] ?? irValue);
                  const displayVal = overridden ? selected.paramOverrides[p.name]
                    : isBaseline ? irValue
                    : baselineVal;
                  const noValue = displayVal === null || displayVal === undefined;
                  return (
                    <div key={p.name} className="flex items-center gap-1.5">
                      <span
                        className={`text-xs w-16 truncate flex-shrink-0 ${
                          overridden ? 'text-accent' : noValue ? 'text-yellow-500' : 'text-gray-500'
                        }`}
                        title={p.name}
                      >
                        {p.name}
                      </span>
                      <input
                        type="number"
                        value={displayVal ?? ''}
                        placeholder={noValue ? 'unset' : undefined}
                        onChange={(e) => {
                          const v = parseFloat(e.target.value);
                          if (!isNaN(v)) setScenarioParam(selected.id, p.name, v);
                        }}
                        className={`w-20 text-xs rounded px-1.5 py-0.5 focus:outline-none focus:ring-1 focus:ring-accent ${
                          overridden
                            ? 'bg-surface-2 border border-accent text-gray-100'
                            : noValue
                            ? 'bg-surface-2 border border-yellow-600/50 text-gray-400 placeholder-yellow-700'
                            : 'bg-surface-2 border border-surface-border text-gray-400'
                        }`}
                        step="any"
                      />
                      {overridden && (
                        <button
                          onClick={() => clearScenarioParam(selected.id, p.name)}
                          className="text-xs text-gray-600 hover:text-red-400 transition-colors leading-none"
                          title={isBaseline ? 'Clear value' : 'Reset to baseline'}
                        >
                          ×
                        </button>
                      )}
                      {!overridden && !isBaseline && !noValue && (
                        <span className="text-xs text-gray-700">baseline</span>
                      )}
                    </div>
                  );
                })}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
