import { useState, useMemo } from 'react';
import {
  ComposedChart, Line, Area, XAxis, YAxis, Tooltip,
  ResponsiveContainer, Legend, CartesianGrid, Brush,
} from 'recharts';
import { useStore } from '../store';
import { buildViews, TRACE_THRESHOLD, type EnsembleMode } from '../lib/buildViews';

// ── Smart legend ───────────────────────────────────────────────────────────────

const LEGEND_GROUP_THRESHOLD = 8;

// eslint-disable-next-line @typescript-eslint/no-explicit-any
interface LegendEntry { value: string; color?: string; type?: string; dataKey?: string | number | ((obj: any) => any) }

function SmartLegend({ payload, groupMap }: { payload?: LegendEntry[]; groupMap?: Record<string, string> }) {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());

  const visible = (payload ?? []).filter((p) => p.value && p.type !== 'none');
  if (!visible.length) return null;

  // Flat view for small legends
  if (visible.length <= LEGEND_GROUP_THRESHOLD) {
    return (
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: '4px 12px', padding: '6px 8px 0', fontSize: 11, fontFamily: 'JetBrains Mono, monospace' }}>
        {visible.map((e) => (
          <span key={typeof e.dataKey === 'string' ? e.dataKey : e.value} style={{ display: 'flex', alignItems: 'center', gap: 5, color: '#9ca3af' }}>
            <span style={{ display: 'inline-block', width: 16, height: 2, background: e.color ?? '#6b7280', borderRadius: 1 }} />
            <span style={{ color: '#d1d5db' }}>{e.value}</span>
          </span>
        ))}
      </div>
    );
  }

  // Grouped view: bucket by explicit group metadata, falling back to the full label
  const groups = new Map<string, LegendEntry[]>();
  for (const e of visible) {
    const dk = typeof e.dataKey === 'string' ? e.dataKey : undefined;
    const k = (dk && groupMap?.[dk]) ?? e.value;
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k)!.push(e);
  }

  const toggle = (k: string) => setExpanded((prev) => {
    const next = new Set(prev);
    next.has(k) ? next.delete(k) : next.add(k);
    return next;
  });

  return (
    <div style={{ padding: '6px 8px 0', fontSize: 11, fontFamily: 'JetBrains Mono, monospace', display: 'flex', flexWrap: 'wrap', gap: '2px 10px' }}>
      {[...groups.entries()].map(([k, entries]) => {
        const isOpen = expanded.has(k);
        const swatch = entries[0].color ?? '#6b7280';
        return (
          <div key={k} style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
            <button
              onClick={() => toggle(k)}
              style={{ display: 'flex', alignItems: 'center', gap: 5, background: 'none', border: 'none', cursor: 'pointer', padding: '1px 0', color: '#9ca3af' }}
            >
              <span style={{ display: 'inline-block', width: 16, height: 2, background: swatch, borderRadius: 1 }} />
              <span style={{ color: '#d1d5db' }}>{k}</span>
              <span style={{ color: '#6b7280', fontSize: 10 }}>({entries.length}) {isOpen ? '▾' : '▸'}</span>
            </button>
            {isOpen && (
              <div style={{ paddingLeft: 21, display: 'flex', flexDirection: 'column', gap: 1 }}>
                {entries.map((e) => (
                  <span key={typeof e.dataKey === 'string' ? e.dataKey : e.value} style={{ color: '#9ca3af', display: 'flex', alignItems: 'center', gap: 4 }}>
                    <span style={{ display: 'inline-block', width: 10, height: 1.5, background: e.color ?? '#6b7280', borderRadius: 1 }} />
                    <span style={{ color: '#6b7280' }}>{e.value.slice(k.length).trim()}</span>
                  </span>
                ))}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

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

function CustomTooltip({ active, payload, label, tooltipKeySet }: {
  active?: boolean;
  payload?: { name: string; value: number; color: string; dataKey: string; type?: string }[];
  label?: number;
  tooltipKeySet?: Set<string>;
}) {
  if (!active || !payload?.length) return null;
  const shown = tooltipKeySet
    ? payload.filter((p) => typeof p.dataKey === 'string' && tooltipKeySet.has(p.dataKey))
    : payload.filter((p) => p.name && p.type !== 'none');
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

  const groupMap = useMemo(() => {
    const m: Record<string, string> = {};
    for (const s of activeView?.series ?? []) {
      if (s.group) m[s.dataKey] = s.group;
    }
    return m;
  }, [activeView]);

  const tooltipKeySet = useMemo(() =>
    new Set((activeView?.series ?? []).filter(s => s.name && !s.hideLegend).map(s => s.dataKey)),
    [activeView]
  );

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
          {/* Band / Lines toggle */}
          {hasResults && (
            <div className="flex items-center gap-0.5 bg-surface-2 rounded p-0.5 ml-2">
              {(['pi', 'traces'] as EnsembleMode[]).map((m) => (
                <button
                  key={m}
                  onClick={() => setModeOverride(modeOverride === m ? null : m)}
                  title={m === 'pi' ? 'P10–P90 quantile band + median' : 'Individual seed traces + mean'}
                  className={`px-2 py-0.5 text-xs rounded transition-colors ${
                    effectiveMode === m
                      ? 'bg-surface-3 text-gray-100'
                      : 'text-gray-500 hover:text-gray-300'
                  }`}
                >
                  {m === 'pi' ? 'Band' : 'Lines'}
                </button>
              ))}
            </div>
          )}
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
              {/* eslint-disable-next-line @typescript-eslint/no-explicit-any */}
              <Tooltip content={(props: any) => <CustomTooltip {...props} tooltipKeySet={tooltipKeySet} />} />
              <Legend content={(props) => <SmartLegend {...props} groupMap={groupMap} />} />
              {activeView.series.map((s) => {
                if (s.kind === 'area_base') {
                  return (
                    <Area
                      key={s.dataKey}
                      name=""
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
                      name=""
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
                    name={s.name}
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
