import { useState, useMemo } from 'react';
import {
  ComposedChart, Line, Area, XAxis, YAxis, Tooltip,
  ResponsiveContainer, Legend, CartesianGrid, Brush,
} from 'recharts';
import { useStore } from '../store';
import { buildViews, TRACE_THRESHOLD, type EnsembleMode } from '../lib/buildViews';

// ── Formatting ────────────────────────────────────────────────────────────────

function fmtCount(v: number): string {
  if (v >= 1_000_000) return (v / 1_000_000).toPrecision(3) + 'M';
  if (v >= 1_000) return (v / 1_000).toPrecision(3) + 'K';
  return v % 1 === 0 ? String(v) : v.toPrecision(3);
}

function fmtTime(v: number): string {
  return v % 1 === 0 ? String(v) : v.toFixed(1);
}

// ── Custom tooltip ────────────────────────────────────────────────────────────

function CustomTooltip({ active, payload, label }: {
  active?: boolean;
  payload?: { name: string; value: number; color: string; dataKey: string }[];
  label?: number;
}) {
  if (!active || !payload?.length) return null;
  // Only show named (legend) series in tooltip
  const shown = payload.filter((p) => p.name);
  if (!shown.length) return null;
  return (
    <div style={{
      background: '#1c2128', border: '1px solid #30363d', borderRadius: 6,
      padding: '8px 10px', fontSize: 11, fontFamily: 'JetBrains Mono, monospace',
      maxWidth: 240,
    }}>
      <div style={{ color: '#9ca3af', marginBottom: 5 }}>t = {fmtTime(label ?? 0)}</div>
      {shown.map((p) => (
        <div key={p.dataKey} style={{ display: 'flex', justifyContent: 'space-between', gap: 16, lineHeight: 1.6 }}>
          <span style={{ color: p.color }}>{p.name}</span>
          <span style={{ color: '#e5e7eb' }}>{fmtCount(p.value)}</span>
        </div>
      ))}
    </div>
  );
}

// ── Main panel ────────────────────────────────────────────────────────────────

export default function ResultsPanel() {
  const ir = useStore((s) => s.ir);
  const scenarios = useStore((s) => s.scenarios);
  const experimentStatus = useStore((s) => s.experimentStatus);

  // Auto-mode: PI if any scenario has >= TRACE_THRESHOLD runs
  const maxRuns = scenarios.reduce((m, s) => Math.max(m, s.runs.length), 0);
  const autoMode: EnsembleMode = maxRuns >= TRACE_THRESHOLD ? 'pi' : 'traces';

  const [modeOverride, setModeOverride] = useState<EnsembleMode | null>(null);
  const effectiveMode: EnsembleMode = modeOverride ?? autoMode;

  const views = useMemo(
    () => (ir ? buildViews(ir, scenarios, effectiveMode) : []),
    [ir, scenarios, effectiveMode]
  );

  const [activeViewId, setActiveViewId] = useState<string | null>(null);
  const activeView = views.find((v) => v.id === activeViewId) ?? views[0] ?? null;

  const isRunning = experimentStatus === 'running';
  const hasResults = views.length > 0;

  return (
    <div className="flex flex-col h-full">
      {/* Top bar: view tabs + mode toggle */}
      {hasResults && (
        <div className="flex items-center gap-1 px-3 py-1 bg-surface-0 border-b border-surface-border flex-shrink-0 overflow-x-auto">
          {views.map((v) => (
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
          <div className="flex-1" />
          {/* PI / Lines toggle */}
          <div className="flex items-center gap-0.5 bg-surface-2 rounded p-0.5 ml-2">
            {(['pi', 'traces'] as EnsembleMode[]).map((m) => (
              <button
                key={m}
                onClick={() => setModeOverride(modeOverride === m ? null : m)}
                title={m === 'pi' ? 'Predictive interval ribbons' : 'Individual trace lines'}
                className={`px-2 py-0.5 text-xs rounded transition-colors ${
                  effectiveMode === m
                    ? 'bg-surface-3 text-gray-100'
                    : 'text-gray-500 hover:text-gray-300'
                }`}
              >
                {m === 'pi' ? 'PI' : 'Lines'}
              </button>
            ))}
          </div>
        </div>
      )}

      {/* Chart area */}
      <div className="flex-1 min-h-0 px-2 py-3">
        {!hasResults && !isRunning && (
          <div className="flex flex-col items-center justify-center h-full gap-2">
            <span className="text-gray-600 text-sm">Click ▶ Run All to simulate</span>
            <span className="text-gray-700 text-xs">Load an example from the header if parameters have no defaults</span>
          </div>
        )}
        {!hasResults && isRunning && (
          <div className="flex items-center justify-center h-full">
            <span className="text-gray-500 text-sm animate-pulse">Simulating…</span>
          </div>
        )}
        {activeView && (
          <ResponsiveContainer width="100%" height="100%">
            <ComposedChart
              data={activeView.data}
              margin={{ top: 4, right: 16, bottom: 4, left: 8 }}
            >
              <CartesianGrid strokeDasharray="3 3" stroke="#1c2128" vertical={false} />
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
                wrapperStyle={{ fontSize: 11, fontFamily: 'JetBrains Mono, monospace', paddingTop: 6, lineHeight: '1.8' }}
                iconType="plainline"
                iconSize={16}
              />
              {activeView.series.map((s) => {
                if (s.kind === 'area_base') {
                  return (
                    <Area
                      key={s.dataKey}
                      type="monotone"
                      dataKey={s.dataKey}
                      stackId={s.stackId}
                      fill="none"
                      stroke="none"
                      isAnimationActive={false}
                      legendType="none"
                    />
                  );
                }
                if (s.kind === 'area_band') {
                  return (
                    <Area
                      key={s.dataKey}
                      type="monotone"
                      dataKey={s.dataKey}
                      stackId={s.stackId}
                      fill={s.color}
                      fillOpacity={s.fillOpacity ?? 0.18}
                      stroke="none"
                      isAnimationActive={false}
                      legendType="none"
                    />
                  );
                }
                return (
                  <Line
                    key={s.dataKey}
                    type="monotone"
                    dataKey={s.dataKey}
                    name={s.name || undefined}
                    stroke={s.color}
                    strokeWidth={s.strokeWidth ?? 2}
                    strokeDasharray={s.strokeDasharray}
                    strokeOpacity={s.strokeOpacity ?? 1}
                    legendType={s.hideLegend || !s.name ? 'none' : 'line'}
                    dot={false}
                    activeDot={s.hideLegend ? false : { r: 3, strokeWidth: 0 }}
                    isAnimationActive={false}
                  />
                );
              })}
              <Brush
                dataKey="t"
                height={18}
                stroke="#30363d"
                fill="#161b22"
                travellerWidth={6}
                tickFormatter={fmtTime}
              />
            </ComposedChart>
          </ResponsiveContainer>
        )}
      </div>
    </div>
  );
}
