import { useState } from 'react';
import { useStore } from '../store';
import { EXAMPLES } from '../lib/examples';

export default function ParamEditor() {
  const ir                  = useStore((s) => s.ir);
  const modelName           = useStore((s) => s.modelName);
  const paramOverrides      = useStore((s) => s.paramOverrides);
  const setParamOverride    = useStore((s) => s.setParamOverride);
  const resetParamOverrides = useStore((s) => s.resetParamOverrides);
  const setSimConfig        = useStore((s) => s.setSimConfig);
  const simConfig           = useStore((s) => s.simConfig);

  const [activePreset, setActivePreset] = useState<string | null>(null);

  if (!ir || ir.parameters.length === 0) {
    return (
      <div className="flex items-center justify-center h-full text-gray-600 text-sm">
        Compile a model to edit parameters
      </div>
    );
  }

  const example      = EXAMPLES.find((e) => e.name === modelName);
  const hasOverrides = Object.keys(paramOverrides).length > 0;

  function applyParamSet(setName: string) {
    const ps = example?.paramSets.find((s) => s.name === setName);
    if (!ps) return;
    setActivePreset(setName);
    resetParamOverrides();
    for (const [k, v] of Object.entries(ps.values)) setParamOverride(k, v);
    if (ps.tEnd != null) setSimConfig({ tEnd: ps.tEnd });
  }

  return (
    <div className="flex flex-col h-full overflow-hidden">
      {/* Toolbar */}
      <div className="flex items-center gap-3 px-4 py-2 border-b border-surface-border flex-shrink-0 bg-surface-1">
        {example && example.paramSets.length > 1 && (
          <>
            <span className="text-xs text-gray-500">preset</span>
            <div className="flex gap-1">
              {example.paramSets.map((ps) => (
                <button
                  key={ps.name}
                  onClick={() => applyParamSet(ps.name)}
                  className={`px-2.5 py-1 text-xs rounded transition-colors ${
                    activePreset === ps.name
                      ? 'bg-accent/15 text-accent border border-accent/30'
                      : 'text-gray-400 border border-surface-border hover:text-gray-200 hover:border-gray-500'
                  }`}
                >
                  {ps.label}
                </button>
              ))}
            </div>
          </>
        )}
        <div className="flex-1" />
        {hasOverrides && (
          <button
            onClick={() => { resetParamOverrides(); setActivePreset(null); }}
            className="text-xs text-gray-500 hover:text-gray-300 transition-colors"
            title="Reset all to model defaults (zero)"
          >
            ↺ reset
          </button>
        )}
      </div>

      {/* Parameter grid */}
      <div className="flex-1 overflow-y-auto p-4">
        <div className="grid gap-3" style={{ gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))' }}>
          {ir.parameters.map((p) => {
            const val        = paramOverrides[p.name] ?? p.value;
            const isOverridden = paramOverrides[p.name] !== undefined;

            return (
              <div
                key={p.name}
                className="rounded-lg border px-3 py-2.5 transition-colors"
                style={{
                  borderColor:     isOverridden ? '#2dd4bf33' : '#30363d',
                  backgroundColor: isOverridden ? '#2dd4bf08' : '#161b22',
                }}
              >
                {/* Name + override indicator */}
                <div className="flex items-baseline justify-between mb-1">
                  <span
                    className="text-sm font-mono font-semibold"
                    style={{ color: isOverridden ? '#2dd4bf' : '#e5e7eb' }}
                  >
                    {p.name}
                  </span>
                  {isOverridden && (
                    <span className="text-xs text-accent/60 ml-2">edited</span>
                  )}
                </div>

                {/* Value input */}
                <input
                  type="number"
                  value={val}
                  onChange={(e) => {
                    const n = parseFloat(e.target.value);
                    if (!isNaN(n)) setParamOverride(p.name, n);
                  }}
                  onKeyDown={(e) => e.stopPropagation()}
                  className="w-full text-sm bg-surface-2 border rounded px-2 py-1 focus:outline-none font-mono transition-colors"
                  style={{
                    borderColor: isOverridden ? '#2dd4bf44' : '#30363d',
                    color:       isOverridden ? '#2dd4bf' : '#d1d5db',
                  }}
                  step="any"
                />

                {/* Default hint */}
                {isOverridden && (
                  <div className="text-xs text-gray-600 mt-1">
                    default: {p.value}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        {/* t_end override */}
        <div className="mt-4 pt-3 border-t border-surface-border">
          <div className="flex items-center gap-3">
            <span className="text-xs text-gray-500 font-mono">t_end</span>
            <input
              type="number"
              value={simConfig.tEnd ?? ''}
              placeholder={`${ir.simulation.t_end} (model default)`}
              onChange={(e) => {
                const n = parseFloat(e.target.value);
                setSimConfig({ tEnd: isNaN(n) ? undefined : n });
              }}
              onKeyDown={(e) => e.stopPropagation()}
              className="w-40 text-sm bg-surface-2 border border-surface-border rounded px-2 py-1 focus:outline-none font-mono text-gray-300 placeholder-gray-600"
              step="any"
              min="0"
            />
            <span className="text-xs text-gray-600">days</span>
          </div>
        </div>
      </div>
    </div>
  );
}
