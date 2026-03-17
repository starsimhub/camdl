import { useState } from 'react';
import { useStore } from '../store';

export default function ScenariosPanel() {
  const ir                  = useStore((s) => s.ir);
  const scenarios           = useStore((s) => s.scenarios);
  const addScenario         = useStore((s) => s.addScenario);
  const removeScenario      = useStore((s) => s.removeScenario);
  const updateScenario      = useStore((s) => s.updateScenario);
  const updateScenarioParam = useStore((s) => s.updateScenarioParam);
  const runScenario         = useStore((s) => s.runScenario);
  const anyRunning          = useStore((s) =>
    s.simStatus === 'running' || s.scenarios.some((sc) => sc.status === 'running')
  );

  // Track which scenario cards are expanded (param editor visible)
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  function toggle(id: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });
  }

  // Badges for collapsed view: params that differ from IR defaults
  function diffBadges(params: Record<string, number>) {
    if (!ir) return [];
    return ir.parameters
      .filter((p) => Math.abs((params[p.name] ?? p.value) - p.value) > 1e-12)
      .map((p) => ({ name: p.name, value: params[p.name] ?? p.value }));
  }

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* Toolbar */}
      <div className="flex items-center gap-2 px-3 py-2 border-b border-surface-border bg-surface-1 flex-shrink-0">
        <button
          onClick={addScenario}
          disabled={!ir}
          className="px-3 py-1 text-xs bg-accent/10 text-accent border border-accent/30 rounded hover:bg-accent/20 disabled:opacity-40 transition-colors"
        >
          + Snapshot baseline
        </button>
        {!ir && <span className="text-xs text-gray-600">compile first</span>}
      </div>

      {/* Scenario list */}
      <div className="flex-1 overflow-y-auto p-2 space-y-1.5">
        {scenarios.length === 0 && (
          <div className="flex items-center justify-center h-full pb-8">
            <span className="text-gray-600 text-sm text-center leading-relaxed">
              No scenarios yet.<br />
              <span className="text-gray-700">Snapshot the Params tab to compare runs.</span>
            </span>
          </div>
        )}

        {scenarios.map((sc) => {
          const isExpanded = expanded.has(sc.id);
          const badges     = diffBadges(sc.params);

          return (
            <div
              key={sc.id}
              className="rounded-md border bg-surface-1 overflow-hidden"
              style={{ borderColor: sc.status === 'ok' ? '#2dd4bf33' : '#30363d' }}
            >
              {/* ── Header row (always visible) ── */}
              <div className="flex items-center gap-1.5 px-2 py-1.5">
                {/* Collapse toggle */}
                <button
                  onClick={() => toggle(sc.id)}
                  className="text-gray-500 hover:text-gray-300 transition-colors text-xs w-4 text-center flex-shrink-0"
                  title={isExpanded ? 'Collapse' : 'Expand params'}
                >
                  {isExpanded ? '▾' : '▸'}
                </button>

                {/* Name */}
                <input
                  value={sc.name}
                  onChange={(e) => updateScenario(sc.id, { name: e.target.value })}
                  onKeyDown={(e) => e.stopPropagation()}
                  className="flex-1 min-w-0 text-xs font-semibold bg-transparent text-gray-200 focus:outline-none border-b border-transparent focus:border-surface-border truncate"
                />

                {/* Status dot */}
                {sc.status === 'ok' && <span className="text-accent text-xs flex-shrink-0">●</span>}
                {sc.status === 'running' && <span className="text-yellow-400 text-xs animate-pulse flex-shrink-0">●</span>}
                {sc.status === 'error' && <span className="text-red-500 text-xs flex-shrink-0">●</span>}

                {/* Run button */}
                <button
                  onClick={() => runScenario(sc.id)}
                  disabled={!ir || anyRunning}
                  title="Run this scenario"
                  className={`px-2 py-0.5 text-xs rounded font-semibold transition-colors disabled:cursor-not-allowed flex-shrink-0 ${
                    sc.status === 'running'
                      ? 'bg-red-500/80 text-white animate-pulse'
                      : anyRunning
                        ? 'bg-surface-2 text-gray-600'
                        : 'bg-accent text-surface-0 hover:bg-accent-dim disabled:opacity-40'
                  }`}
                >
                  {sc.status === 'running' ? '●' : '▶'}
                </button>

                {/* Delete */}
                <button
                  onClick={() => removeScenario(sc.id)}
                  title="Remove scenario"
                  className="text-gray-600 hover:text-red-400 transition-colors text-xs leading-none flex-shrink-0"
                >
                  ✕
                </button>
              </div>

              {/* ── Collapsed summary: diff badges ── */}
              {!isExpanded && (
                <div className="flex flex-wrap gap-1 px-3 pb-1.5">
                  {badges.length === 0 ? (
                    <span className="text-xs text-gray-600 italic">baseline</span>
                  ) : (
                    badges.map(({ name, value }) => (
                      <span key={name} className="text-xs px-1.5 py-0.5 rounded bg-surface-2 text-accent/80 font-mono">
                        {name}={value}
                      </span>
                    ))
                  )}
                  {sc.tEnd != null && (
                    <span className="text-xs px-1.5 py-0.5 rounded bg-surface-2 text-gray-400 font-mono">
                      t_end={sc.tEnd}
                    </span>
                  )}
                  {sc.status === 'ok' && (
                    <span className="text-xs text-gray-600 ml-auto">
                      {sc.trajectories.length}× rep
                    </span>
                  )}
                </div>
              )}

              {/* ── Expanded: inline param editor ── */}
              {isExpanded && ir && (
                <div className="px-2 pb-2 pt-1 border-t border-surface-border/50">
                  {/* Param grid */}
                  <div
                    className="grid gap-x-3 gap-y-1.5"
                    style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(140px, 1fr))' }}
                  >
                    {ir.parameters.map((p) => {
                      const val         = sc.params[p.name] ?? p.value;
                      const isDiff      = Math.abs(val - p.value) > 1e-12;
                      return (
                        <div key={p.name}>
                          <div className="flex items-baseline justify-between mb-0.5">
                            <span
                              className="text-xs font-mono"
                              style={{ color: isDiff ? '#2dd4bf' : '#9ca3af' }}
                            >
                              {p.name}
                            </span>
                            {isDiff && (
                              <button
                                onClick={() => updateScenarioParam(sc.id, p.name, p.value)}
                                title="Reset to IR default"
                                className="text-xs text-gray-700 hover:text-gray-400 ml-1"
                              >
                                ↺
                              </button>
                            )}
                          </div>
                          <input
                            type="number"
                            value={val}
                            onChange={(e) => {
                              const n = parseFloat(e.target.value);
                              if (!isNaN(n)) updateScenarioParam(sc.id, p.name, n);
                            }}
                            onKeyDown={(e) => e.stopPropagation()}
                            className="w-full text-xs bg-surface-2 border rounded px-1.5 py-0.5 focus:outline-none font-mono transition-colors"
                            style={{
                              borderColor: isDiff ? '#2dd4bf44' : '#30363d',
                              color:       isDiff ? '#2dd4bf'   : '#d1d5db',
                            }}
                            step="any"
                          />
                        </div>
                      );
                    })}
                  </div>

                  {/* Run settings row */}
                  <div className="flex flex-wrap items-center gap-x-3 gap-y-1 mt-2 pt-2 border-t border-surface-border/50">
                    <span className="text-xs text-gray-600 font-mono">t_end</span>
                    <input
                      type="number"
                      value={sc.tEnd ?? ''}
                      placeholder={`${ir.simulation.t_end}`}
                      onChange={(e) => {
                        const n = parseFloat(e.target.value);
                        updateScenario(sc.id, { tEnd: isNaN(n) ? undefined : n });
                      }}
                      onKeyDown={(e) => e.stopPropagation()}
                      className="w-20 text-xs bg-surface-2 border border-surface-border rounded px-1.5 py-0.5 focus:outline-none font-mono text-gray-300 placeholder-gray-600"
                      step="any" min="0"
                    />
                    <span className="text-xs text-gray-600 font-mono">seed</span>
                    <input
                      type="number"
                      value={sc.seed}
                      min={0}
                      onChange={(e) => updateScenario(sc.id, { seed: parseInt(e.target.value) || 0 })}
                      onKeyDown={(e) => e.stopPropagation()}
                      className="w-16 text-xs bg-surface-2 border border-surface-border rounded px-1.5 py-0.5 focus:outline-none font-mono text-gray-300"
                    />
                    <span className="text-xs text-gray-600 font-mono">reps</span>
                    <input
                      type="number"
                      value={sc.replicates}
                      min={1} max={50}
                      onChange={(e) => updateScenario(sc.id, { replicates: Math.max(1, parseInt(e.target.value) || 1) })}
                      onKeyDown={(e) => e.stopPropagation()}
                      className="w-14 text-xs bg-surface-2 border border-surface-border rounded px-1.5 py-0.5 focus:outline-none font-mono text-gray-300"
                    />
                  </div>

                  {/* Status line */}
                  {sc.status === 'ok' && (
                    <div className="text-xs text-gray-600 mt-1.5">
                      <span className="text-accent">●</span>{' '}
                      {sc.trajectories.length} replicate{sc.trajectories.length !== 1 ? 's' : ''} — see Compare in Run tab
                    </div>
                  )}
                  {sc.status === 'error' && (
                    <div className="text-xs text-red-400 mt-1.5 font-mono break-all">{sc.error}</div>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
