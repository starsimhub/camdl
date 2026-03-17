import { useState, useMemo } from 'react';
import {
  LineChart, Line, XAxis, YAxis, Tooltip,
  ResponsiveContainer, Legend, CartesianGrid, Brush,
} from 'recharts';
import { useStore } from '../store';
import { buildViews, buildCompareViews } from '../lib/buildViews';

// ── Formatting helpers ────────────────────────────────────────────────────────

function fmtCount(v: number): string {
  if (v >= 1_000_000) return (v / 1_000_000).toPrecision(3) + 'M';
  if (v >= 1_000)     return (v / 1_000).toPrecision(3) + 'K';
  return v % 1 === 0 ? String(v) : v.toPrecision(3);
}

function fmtTime(v: number): string {
  if (v % 1 === 0) return String(v);
  return v.toFixed(1);
}

// ── Custom tooltip ────────────────────────────────────────────────────────────

function CustomTooltip({ active, payload, label }: {
  active?: boolean;
  payload?: { name: string; value: number; color: string }[];
  label?: number;
}) {
  if (!active || !payload?.length) return null;
  return (
    <div style={{
      background: '#1c2128', border: '1px solid #30363d', borderRadius: 6,
      padding: '8px 10px', fontSize: 11, fontFamily: 'JetBrains Mono, monospace',
      maxWidth: 220,
    }}>
      <div style={{ color: '#9ca3af', marginBottom: 5 }}>t = {fmtTime(label ?? 0)}</div>
      {payload.map((p) => (
        <div key={p.name} style={{ display: 'flex', justifyContent: 'space-between', gap: 16, lineHeight: 1.6 }}>
          <span style={{ color: p.color }}>{p.name}</span>
          <span style={{ color: '#e5e7eb' }}>{fmtCount(p.value)}</span>
        </div>
      ))}
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────────────────

export default function RunPanel() {
  const trajectory    = useStore((s) => s.trajectory);
  const simStatus     = useStore((s) => s.simStatus);
  const simError      = useStore((s) => s.simError);
  const simConfig     = useStore((s) => s.simConfig);
  const setSimConfig  = useStore((s) => s.setSimConfig);
  const runSimulation = useStore((s) => s.runSimulation);
  const ir            = useStore((s) => s.ir);
  const scenarios     = useStore((s) => s.scenarios);
  const anyRunning    = useStore((s) =>
    s.simStatus === 'running' || s.scenarios.some((sc) => sc.status === 'running')
  );

  const views = useMemo(
    () => (ir && trajectory ? buildViews(ir, trajectory) : []),
    [ir, trajectory]
  );

  const compareViews = useMemo(
    () => buildCompareViews(scenarios),
    [scenarios]
  );

  const allViews = useMemo(() => [...views, ...compareViews], [views, compareViews]);

  const [activeViewId, setActiveViewId] = useState<string | null>(null);

  // Pick active view: user selection → first view → null
  const activeView = allViews.find((v) => v.id === activeViewId) ?? allViews[0] ?? null;

  // Reset to first view when trajectory changes
  useMemo(() => { setActiveViewId(null); }, [trajectory]);

  return (
    <div className="flex flex-col h-full">
      {/* Config bar */}
      <div className="flex items-center gap-3 px-4 py-2 bg-surface-1 border-b border-surface-border flex-shrink-0">
        <span className="text-xs text-gray-500">backend</span>
        <select
          value={simConfig.backend}
          onChange={(e) => setSimConfig({ backend: e.target.value as 'gillespie' })}
          className="text-xs bg-surface-2 border border-surface-border text-gray-300 rounded px-2 py-0.5 focus:outline-none"
        >
          <option value="gillespie">Gillespie (SSA)</option>
          <option value="tau_leap">Tau-leap</option>
          <option value="chain_binomial">Chain-binomial</option>
        </select>

        {(simConfig.backend === 'tau_leap' || simConfig.backend === 'chain_binomial') && (
          <>
            <span className="text-xs text-gray-500">dt</span>
            <input
              type="number"
              value={simConfig.dt ?? 1}
              onChange={(e) => setSimConfig({ dt: parseFloat(e.target.value) })}
              className="w-16 text-xs bg-surface-2 border border-surface-border text-gray-300 rounded px-2 py-0.5 focus:outline-none"
              step="0.1" min="0.01"
            />
          </>
        )}

        <span className="text-xs text-gray-500">seed</span>
        <input
          type="number"
          value={simConfig.seed}
          onChange={(e) => setSimConfig({ seed: parseInt(e.target.value) })}
          className="w-16 text-xs bg-surface-2 border border-surface-border text-gray-300 rounded px-2 py-0.5 focus:outline-none"
          min="0"
        />

        <div className="flex-1" />

        <button
          onClick={runSimulation}
          disabled={!ir || anyRunning}
          className={`px-3 py-1 text-xs rounded font-semibold transition-colors disabled:cursor-not-allowed ${
            anyRunning
              ? 'bg-red-500/80 text-white animate-pulse'
              : 'bg-accent text-surface-0 hover:bg-accent-dim disabled:opacity-40'
          }`}
        >
          {simStatus === 'running' ? '● Running…' : anyRunning ? '● Running…' : '▶ Run'}
        </button>
      </div>

      {/* View tabs — only shown when we have results */}
      {allViews.length > 0 && (
        <div className="flex items-center gap-1 px-3 py-1 bg-surface-0 border-b border-surface-border flex-shrink-0 overflow-x-auto">
          {allViews.map((v) => (
            <button
              key={v.id}
              onClick={() => setActiveViewId(v.id)}
              title={v.description}
              className={`px-2.5 py-0.5 text-xs rounded transition-colors ${
                v.id === (activeView?.id)
                  ? 'bg-surface-3 text-gray-100'
                  : 'text-gray-500 hover:text-gray-300'
              }`}
            >
              {v.label}
            </button>
          ))}
        </div>
      )}

      {/* Chart area */}
      <div className="flex-1 min-h-0 px-2 py-3">
        {simStatus === 'error' && (
          <div className="flex items-center justify-center h-full px-4">
            <span className="text-red-400 text-sm text-center">{simError}</span>
          </div>
        )}
        {/* Placeholder: only shown when there is nothing to display at all */}
        {allViews.length === 0 && !anyRunning && simStatus !== 'error' && (
          <div className="flex items-center justify-center h-full">
            <span className="text-gray-600 text-sm">Click ▶ Run to simulate</span>
          </div>
        )}
        {allViews.length === 0 && anyRunning && (
          <div className="flex items-center justify-center h-full">
            <span className="text-gray-500 text-sm animate-pulse">Simulating…</span>
          </div>
        )}
        {activeView && (
          <ResponsiveContainer width="100%" height="100%">
            <LineChart
              data={activeView.data}
              margin={{ top: 4, right: 16, bottom: 4, left: 8 }}
            >
              <CartesianGrid
                strokeDasharray="3 3"
                stroke="#1c2128"
                vertical={false}
              />
              <XAxis
                dataKey="t"
                tickFormatter={fmtTime}
                tick={{ fontSize: 10, fill: '#6b7280', fontFamily: 'JetBrains Mono, monospace' }}
                axisLine={{ stroke: '#30363d' }}
                tickLine={false}
                label={{ value: 'time', position: 'insideBottomRight', offset: -4, fontSize: 10, fill: '#4b5563' }}
              />
              <YAxis
                tickFormatter={fmtCount}
                tick={{ fontSize: 10, fill: '#6b7280', fontFamily: 'JetBrains Mono, monospace' }}
                axisLine={false}
                tickLine={false}
                width={44}
              />
              <Tooltip content={<CustomTooltip />} />
              <Legend
                wrapperStyle={{
                  fontSize: 11,
                  fontFamily: 'JetBrains Mono, monospace',
                  paddingTop: 6,
                  lineHeight: '1.8',
                }}
                iconType="plainline"
                iconSize={16}
              />
              {activeView.series.map((s) => (
                <Line
                  key={s.dataKey}
                  type="monotone"
                  dataKey={s.dataKey}
                  name={s.name}
                  stroke={s.color}
                  strokeWidth={s.strokeWidth ?? 2}
                  strokeDasharray={s.strokeDasharray}
                  strokeOpacity={s.strokeOpacity ?? 1}
                  legendType={s.hideLegend ? 'none' : 'line'}
                  dot={false}
                  activeDot={s.hideLegend ? false : { r: 3, strokeWidth: 0 }}
                  isAnimationActive={false}
                />
              ))}
              <Brush
                dataKey="t"
                height={18}
                stroke="#30363d"
                fill="#161b22"
                travellerWidth={6}
                tickFormatter={fmtTime}
              />
            </LineChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}
