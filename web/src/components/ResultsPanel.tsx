import { useState, useMemo, useEffect, useRef, Component, type ReactNode, type ErrorInfo } from 'react';
import {
  ComposedChart, Line, Area, XAxis, YAxis, Tooltip,
  ResponsiveContainer, Legend, CartesianGrid, Brush,
} from 'recharts';
import { useStore } from '../store';
import { buildViews, findDynamicEndIndex, TRACE_THRESHOLD, type EnsembleMode } from '../lib/buildViews';
import MapPanel from './MapPanel';
import { detectPatches } from '../lib/patchStats';

// ── Map error boundary ─────────────────────────────────────────────────────────

class MapErrorBoundary extends Component<{ children: ReactNode }, { error: string | null }> {
  state = { error: null };
  static getDerivedStateFromError(e: Error) { return { error: e.message }; }
  componentDidCatch(e: Error, info: ErrorInfo) {
    console.error('[MapPanel crash]', e, info.componentStack);
  }
  render() {
    if (this.state.error) {
      return (
        <div className="flex flex-col items-center justify-center h-full gap-2 text-sm">
          <span className="text-red-500">Map failed to render</span>
          <span className="text-xs text-gray-400 font-mono max-w-md text-center">{this.state.error}</span>
          <button
            onClick={() => this.setState({ error: null })}
            className="text-xs px-2 py-0.5 border border-gray-300 rounded hover:border-gray-500"
          >
            Retry
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

// ── Brush handle ──────────────────────────────────────────────────────────────

function BrushHandle(props: Record<string, unknown>) {
  const x = props.x as number ?? 0;
  const y = props.y as number ?? 0;
  const width = props.width as number ?? 8;
  const height = props.height as number ?? 18;
  const cx = x + width / 2;
  return (
    <g>
      <rect x={x - 1} y={y} width={width + 2} height={height} fill="rgb(var(--accent-rgb))" rx={2} opacity={0.9} />
      <line x1={cx - 2} y1={y + 4} x2={cx - 2} y2={y + height - 4} stroke="white" strokeWidth={1} strokeOpacity={0.5} />
      <line x1={cx + 2} y1={y + 4} x2={cx + 2} y2={y + height - 4} stroke="white" strokeWidth={1} strokeOpacity={0.5} />
    </g>
  );
}

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
          <span key={typeof e.dataKey === 'string' ? e.dataKey : e.value} style={{ display: 'flex', alignItems: 'center', gap: 5, color: 'var(--text-md)' }}>
            <span style={{ display: 'inline-block', width: 16, height: 2, background: e.color ?? 'var(--text-lo)', borderRadius: 1 }} />
            <span style={{ color: 'var(--text-hi)' }}>{e.value}</span>
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
        const swatch = entries[0].color ?? 'var(--text-lo)';
        return (
          <div key={k} style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
            <button
              onClick={() => toggle(k)}
              style={{ display: 'flex', alignItems: 'center', gap: 5, background: 'none', border: 'none', cursor: 'pointer', padding: '1px 0', color: 'var(--text-md)' }}
            >
              <span style={{ display: 'inline-block', width: 16, height: 2, background: swatch, borderRadius: 1 }} />
              <span style={{ color: 'var(--text-hi)' }}>{k}</span>
              <span style={{ color: 'var(--text-lo)', fontSize: 10 }}>({entries.length}) {isOpen ? '▾' : '▸'}</span>
            </button>
            {isOpen && (
              <div style={{ paddingLeft: 21, display: 'flex', flexDirection: 'column', gap: 1 }}>
                {entries.map((e) => (
                  <span key={typeof e.dataKey === 'string' ? e.dataKey : e.value} style={{ color: 'var(--text-md)', display: 'flex', alignItems: 'center', gap: 4 }}>
                    <span style={{ display: 'inline-block', width: 10, height: 1.5, background: e.color ?? 'var(--text-lo)', borderRadius: 1 }} />
                    <span style={{ color: 'var(--text-lo)' }}>{e.value.slice(k.length).trim()}</span>
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
      background: 'var(--surface-2)', border: '1px solid var(--border)', borderRadius: 6,
      padding: '8px 10px', fontSize: 11, fontFamily: 'JetBrains Mono, monospace',
      maxWidth: 240,
    }}>
      <div style={{ color: 'var(--text-md)', marginBottom: 5 }}>t = {fmtTime(label ?? 0)}</div>
      {shown.map((p) => (
        <div key={p.dataKey} style={{ display: 'flex', justifyContent: 'space-between', gap: 16, lineHeight: 1.6 }}>
          <span style={{ color: p.color }}>{p.name}</span>
          <span style={{ color: 'var(--text-hi)' }}>{fmtCount(p.value)}</span>
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
  const [showMap, setShowMap] = useState(false);
  const activeView = showMap ? null : (views.find((v) => v.id === activeViewId) ?? views[0] ?? null);

  // Detect whether the loaded model has patch stratification
  const firstTraj = scenarios.flatMap((s) => s.runs).find(Boolean)?.trajectory;
  const hasPatchModel = useMemo(
    () => (firstTraj ? detectPatches(firstTraj, ir) !== null : false),
    [firstTraj, ir],
  );

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

  // Per-view brush end index. Auto-computed from each view's own data on new
  // runs; user drag overrides per-view. Reset all when run count changes.
  const [brushEnds, setBrushEnds] = useState<Record<string, number | undefined>>({});
  const prevRunCount = useRef(0);

  // Compute dynamic end for each view whenever views change
  const dynamicEnds = useMemo(() => {
    const result: Record<string, number | undefined> = {};
    for (const v of views) result[v.id] = findDynamicEndIndex(v.data);
    return result;
  }, [views]);

  useEffect(() => {
    const totalRuns = scenarios.reduce((sum, s) => sum + s.runs.length, 0);
    if (totalRuns > 0 && totalRuns !== prevRunCount.current) {
      prevRunCount.current = totalRuns;
      setBrushEnds(dynamicEnds);
    }
  }, [scenarios, dynamicEnds]);

  const isRunning = experimentStatus === 'running';
  const hasResults = views.length > 0;

  return (
    <div className="flex flex-col h-full">
      {/* Top bar: view tabs + mode toggle */}
      {hasResults && (
        <div className="flex items-center gap-1 px-3 py-1 bg-white border-b border-gray-200 flex-shrink-0 overflow-x-auto dark:bg-surface-0 dark:border-surface-border">
          {views.map((v) => (
            <button
              key={v.id}
              onClick={() => { setActiveViewId(v.id); setShowMap(false); }}
              title={v.description}
              className={`px-2.5 py-0.5 text-xs rounded transition-colors ${
                !showMap && v.id === (activeView?.id ?? views[0]?.id)
                  ? 'bg-gray-200 text-gray-900 dark:bg-surface-3 dark:text-gray-100'
                  : 'text-gray-500 hover:text-gray-700 dark:text-gray-500 dark:hover:text-gray-300'
              }`}
            >
              {v.label}
            </button>
          ))}
          {hasPatchModel && (
            <button
              onClick={() => setShowMap(true)}
              title="Geographic choropleth by patch"
              className={`px-2.5 py-0.5 text-xs rounded transition-colors ${
                showMap
                  ? 'bg-gray-200 text-gray-900 dark:bg-surface-3 dark:text-gray-100'
                  : 'text-gray-500 hover:text-gray-700 dark:text-gray-500 dark:hover:text-gray-300'
              }`}
            >
              Map
            </button>
          )}
          <div className="flex-1" />
          {/* Band / Lines toggle */}
          {hasResults && (
            <div className="flex items-center gap-0.5 bg-gray-100 rounded p-0.5 ml-2 dark:bg-surface-2">
              {(['pi', 'traces'] as EnsembleMode[]).map((m) => (
                <button
                  key={m}
                  onClick={() => setModeOverride(modeOverride === m ? null : m)}
                  title={m === 'pi' ? 'P10–P90 quantile band + median' : 'Individual seed traces + mean'}
                  className={`px-2 py-0.5 text-xs rounded transition-colors ${
                    effectiveMode === m
                      ? 'bg-gray-200 text-gray-900 dark:bg-surface-3 dark:text-gray-100'
                      : 'text-gray-500 hover:text-gray-700 dark:text-gray-500 dark:hover:text-gray-300'
                  }`}
                >
                  {m === 'pi' ? 'Band' : 'Lines'}
                </button>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Map panel */}
      {showMap && <div className="flex-1 min-h-0"><MapErrorBoundary><MapPanel /></MapErrorBoundary></div>}

      {/* Chart area */}
      {!showMap && <div className="flex-1 min-h-0 px-2 py-3">
        {!hasResults && !isRunning && (
          <div className="flex flex-col items-center justify-center h-full gap-2">
            <span className="text-gray-500 text-sm dark:text-gray-600">Click ▶ Run All to simulate</span>
            <span className="text-gray-400 text-xs dark:text-gray-700">Load an example from the header if parameters have no defaults</span>
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
              margin={{ top: 4, right: 16, bottom: 28, left: 8 }}
            >
              <CartesianGrid strokeDasharray="3 3" stroke="var(--surface-2)" vertical={false} />
              <XAxis
                dataKey="t"
                tickFormatter={fmtTime}
                tick={{ fontSize: 10, fill: 'var(--text-lo)', fontFamily: 'JetBrains Mono, monospace' }}
                axisLine={{ stroke: 'var(--border)' }}
                tickLine={false}
                label={{ value: 'time', position: 'insideBottomRight', offset: -4, fontSize: 10, fill: 'var(--text-lo)' }}
              />
              <YAxis
                tickFormatter={fmtCount}
                tick={{ fontSize: 10, fill: 'var(--text-lo)', fontFamily: 'JetBrains Mono, monospace' }}
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
                stroke="var(--border)"
                fill="var(--surface-1)"
                travellerWidth={8}
                // eslint-disable-next-line @typescript-eslint/no-explicit-any
                traveller={BrushHandle as any}
                tickFormatter={fmtTime}
                startIndex={0}
                endIndex={activeView ? brushEnds[activeView.id] : undefined}
                onChange={(range: { startIndex?: number; endIndex?: number }) => {
                  if (!activeView || range.endIndex === undefined) return;
                  setBrushEnds((prev) => ({ ...prev, [activeView.id]: range.endIndex }));
                }}
              />
            </ComposedChart>
          </ResponsiveContainer>
        )}
      </div>}
    </div>
  );
}
