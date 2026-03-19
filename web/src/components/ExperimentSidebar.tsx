import { useState, useRef, useEffect } from 'react';
import { useStore } from '../store';
import { EXAMPLES } from '../lib/examples';

export default function ExperimentSidebar() {
  const ir = useStore((s) => s.ir);
  const scenarios = useStore((s) => s.scenarios);
  const runConfig = useStore((s) => s.runConfig);
  const modelName = useStore((s) => s.modelName);
  const addScenario = useStore((s) => s.addScenario);
  const addPresetScenario = useStore((s) => s.addPresetScenario);
  const removeScenario = useStore((s) => s.removeScenario);
  const renameScenario = useStore((s) => s.renameScenario);
  const setScenarioParam = useStore((s) => s.setScenarioParam);
  const clearScenarioParam = useStore((s) => s.clearScenarioParam);

  const [selectedId, setSelectedId] = useState<string>(() => scenarios[0]?.id ?? '');
  const [editingName, setEditingName] = useState<string | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);

  const selected = scenarios.find((s) => s.id === selectedId) ?? scenarios[0];
  const isBaseline = selected?.id === scenarios[0]?.id;

  const examplePresets = EXAMPLES.find((e) => e.name === modelName)?.paramSets ?? [];

  const irParams = ir?.parameters ?? [];

  // Close menu on outside click
  useEffect(() => {
    if (!menuOpen) return;
    const handler = (e: MouseEvent) => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        setMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [menuOpen]);

  return (
    <div className="flex flex-col h-full overflow-hidden bg-surface-1 border-r border-surface-border">

      {/* ── Scenarios header ──────────────────────────────────────────────────── */}
      <div className="px-3 pt-3 pb-2 border-b border-surface-border flex-shrink-0">
        <div className="flex items-center justify-between mb-2">
          <span className="text-xs text-gray-500 font-medium uppercase tracking-wider">Scenarios</span>

          {/* Add scenario dropdown */}
          <div className="relative" ref={menuRef}>
            <button
              disabled={!ir}
              onClick={() => setMenuOpen((o) => !o)}
              className="flex items-center gap-1 px-2 py-0.5 text-xs text-gray-400 hover:text-gray-200 border border-surface-border rounded transition-colors disabled:opacity-40"
            >
              + Add <span className="text-gray-600">▾</span>
            </button>
            {menuOpen && (
              <div className="absolute right-0 top-full mt-1 z-50 min-w-[140px] bg-surface-2 border border-surface-border rounded shadow-lg py-0.5">
                {examplePresets.length > 0 && (
                  <>
                    <div className="px-2.5 py-1 text-xs text-gray-600 uppercase tracking-wider">Presets</div>
                    {examplePresets.map((p) => (
                      <button
                        key={p.name}
                        onClick={() => { addPresetScenario(p.name); setMenuOpen(false); }}
                        className="w-full text-left px-2.5 py-1 text-xs text-gray-300 hover:bg-surface-3 hover:text-gray-100 transition-colors"
                      >
                        {p.label}
                      </button>
                    ))}
                    <div className="my-0.5 border-t border-surface-border" />
                  </>
                )}
                <button
                  onClick={() => { addScenario(true); setMenuOpen(false); }}
                  className="w-full text-left px-2.5 py-1 text-xs text-gray-300 hover:bg-surface-3 hover:text-gray-100 transition-colors"
                >
                  From baseline
                </button>
                <button
                  onClick={() => { addScenario(false); setMenuOpen(false); }}
                  className="w-full text-left px-2.5 py-1 text-xs text-gray-300 hover:bg-surface-3 hover:text-gray-100 transition-colors"
                >
                  Clone last
                </button>
              </div>
            )}
          </div>
        </div>

        {/* Scenario chips */}
        <div className="flex flex-wrap gap-1.5">
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
