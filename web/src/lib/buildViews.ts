import type { IrModel, ModelStructure } from '../types/ir';
import type { TrajectoryJson } from '../types/trajectory';
import type { Scenario } from '../types/experiment';
import { compartmentColor } from './irToCanvas';

// ── Public types ──────────────────────────────────────────────────────────────

export type EnsembleMode = 'pi' | 'traces';

export const TRACE_THRESHOLD = 6;

export interface ChartSeries {
  dataKey: string;
  name: string;
  color: string;
  /** 'line' → recharts Line; 'area_base' / 'area_band' → recharts Area for PI ribbon. */
  kind: 'line' | 'area_base' | 'area_band';
  stackId?: string;
  strokeDasharray?: string;
  strokeOpacity?: number;
  strokeWidth?: number;
  fillOpacity?: number;
  hideLegend?: boolean;
  /** Compartment base or variable family — used by SmartLegend for collapsible grouping. */
  group?: string;
}

export interface ChartPoint {
  t: number;
  [key: string]: number;
}

export interface PlotView {
  id: string;
  label: string;
  description: string;
  data: ChartPoint[];
  series: ChartSeries[];
}

// ── Internals ─────────────────────────────────────────────────────────────────

function quantile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  if (sorted.length === 1) return sorted[0];
  const pos = q * (sorted.length - 1);
  const lo = Math.floor(pos);
  const hi = Math.ceil(pos);
  if (lo === hi) return sorted[lo];
  return sorted[lo] * (hi - pos) + sorted[hi] * (pos - lo);
}

function computeStats(values: number[]): { lo: number; band: number; median: number; mean: number } {
  if (values.length === 0) return { lo: 0, band: 0, median: 0, mean: 0 };
  const sorted = [...values].sort((a, b) => a - b);
  const lo = Math.max(0, quantile(sorted, 0.1));
  const hi = quantile(sorted, 0.9);
  return {
    lo,
    band: Math.max(0, hi - lo),
    median: quantile(sorted, 0.5),
    mean: values.reduce((a, b) => a + b, 0) / values.length,
  };
}

/**
 * Build a fast getter for summing named compartments from a trajectory.
 * Pre-resolves name→index once; returned function is O(k) where k = compNames.length.
 */
function makeSumGetter(
  traj: TrajectoryJson,
  compNames: string[],
): (snapIdx: number) => number {
  const nInt = traj.int_compartment_names.length;
  const nameMap = new Map<string, number>();
  traj.int_compartment_names.forEach((n, i) => nameMap.set(n, i));
  traj.real_compartment_names.forEach((n, i) => nameMap.set(n, nInt + i));
  const resolved: { isInt: boolean; idx: number }[] = [];
  for (const name of compNames) {
    const flatIdx = nameMap.get(name);
    if (flatIdx !== undefined) {
      resolved.push({ isInt: flatIdx < nInt, idx: flatIdx < nInt ? flatIdx : flatIdx - nInt });
    }
  }
  const snapshots = traj.snapshots;
  const len = snapshots.length;
  return (snapIdx: number) => {
    const snap = snapshots[Math.min(snapIdx, len - 1)];
    if (!snap) return 0;
    let sum = 0;
    for (const { isInt, idx } of resolved) {
      sum += isInt ? (snap.counts[idx] ?? 0) : (snap.values[idx] ?? 0);
    }
    return sum;
  };
}

/**
 * Returns a (traj, snapIdx) → number function that pre-resolves indices on first
 * call per trajectory, then reuses them. Drop-in replacement for the old sumAtIdx.
 */
function makeCachedSumGetter(members: string[]): (traj: TrajectoryJson, si: number) => number {
  const cache = new WeakMap<TrajectoryJson, (si: number) => number>();
  return (traj, si) => {
    let g = cache.get(traj);
    if (!g) { g = makeSumGetter(traj, members); cache.set(traj, g); }
    return g(si);
  };
}

/** Get flow value for a named transition at snapshot index. */
function flowAtIdx(traj: TrajectoryJson, snapIdx: number, transName: string): number {
  const si = Math.min(snapIdx, traj.snapshots.length - 1);
  const snap = traj.snapshots[si];
  if (!snap) return 0;
  const idx = traj.transition_names.indexOf(transName);
  return idx >= 0 ? (snap.flows[idx] ?? 0) : 0;
}

/**
 * Find the last "active" index in a view's data array by computing rolling
 * variance of all numeric signals, normalised so each contributes equally.
 * Returns the index where dynamics have effectively ended + 10% tail buffer,
 * or undefined if the data is trivially flat.
 */
export function findDynamicEndIndex(data: ChartPoint[]): number | undefined {
  const n = data.length;
  if (n < 2) return undefined;

  // All numeric keys except the time axis
  const keys = Object.keys(data[0]).filter((k) => k !== 't');
  if (!keys.length) return undefined;

  // Build per-signal time series
  const sigMat: number[][] = keys.map((k) => data.map((row) => (row[k] as number | undefined) ?? 0));

  // Normalise each signal by its range so all signals contribute equally
  const normalised = sigMat.map((sig) => {
    const min = Math.min(...sig);
    const max = Math.max(...sig);
    const range = max - min || 1;
    return sig.map((v) => (v - min) / range);
  });

  // Rolling variance of each normalised signal, summed across all signals
  const W = Math.max(5, Math.floor(n * 0.05));
  const activity = new Array(n).fill(0);
  for (let i = W; i < n; i++) {
    let total = 0;
    for (const sig of normalised) {
      const window = sig.slice(i - W, i + 1);
      const mean = window.reduce((a, b) => a + b, 0) / window.length;
      total += window.reduce((a, v) => a + (v - mean) ** 2, 0) / window.length;
    }
    activity[i] = total;
  }

  const maxActivity = Math.max(...activity);
  if (maxActivity === 0) return undefined;
  const threshold = maxActivity * 0.01;

  let lastActive = W;
  for (let i = n - 1; i >= W; i--) {
    if (activity[i] >= threshold) { lastActive = i; break; }
  }

  return Math.min(n - 1, Math.floor(lastActive * 1.1));
}

// Dash patterns for multi-variable views — one per scenario / stratum
const SCENARIO_DASHES = ['', '6 3', '2 3', '10 4 2 4', '1 4'];

/**
 * Build a multi-variable view (Aggregate, All, By Dim, Flows).
 * Band mode: P10–P90 ribbon + median per variable × scenario.
 * Lines mode: individual seed traces (faint) + mean per variable × scenario.
 * Color = variable, dash = scenario.
 */
function buildMultiVarView(
  id: string,
  label: string,
  description: string,
  active: Scenario[],
  timeGrid: number[],
  variables: { key: string; label: string; color: string; group?: string; strokeDasharray?: string; getVal: (traj: TrajectoryJson, si: number) => number }[],
  mode: EnsembleMode,
): PlotView {
  const data: ChartPoint[] = timeGrid.map((t, ti) => {
    const pt: ChartPoint = { t };
    for (const [scIdx, sc] of active.entries()) {
      for (const v of variables) {
        const values = sc.runs.map((r) => v.getVal(r.trajectory, ti));
        if (mode === 'pi') {
          const stats = computeStats(values);
          pt[`s${scIdx}_${v.key}_lo`] = stats.lo;
          pt[`s${scIdx}_${v.key}_band`] = stats.band;
          pt[`s${scIdx}_${v.key}_median`] = stats.median;
        } else {
          pt[`s${scIdx}_${v.key}_mean`] =
            values.length > 0 ? values.reduce((a, b) => a + b, 0) / values.length : 0;
          values.forEach((val, ri) => { pt[`s${scIdx}_${v.key}_r${ri}`] = val; });
        }
      }
    }
    return pt;
  });

  const series: ChartSeries[] = [];
  for (const [scIdx, sc] of active.entries()) {
    const dash = SCENARIO_DASHES[scIdx % SCENARIO_DASHES.length];
    for (const v of variables) {
      const seriesDash = dash || v.strokeDasharray || undefined;
      const legendName = active.length > 1 ? `${v.label} · ${sc.name}` : v.label;
      if (mode === 'pi') {
        const sid = `s${scIdx}_${v.key}`;
        series.push({ dataKey: `${sid}_lo`,     name: '',         color: v.color, kind: 'area_base', stackId: sid, hideLegend: true });
        series.push({ dataKey: `${sid}_band`,   name: '',         color: v.color, kind: 'area_band', stackId: sid, fillOpacity: 0.15, hideLegend: true });
        series.push({ dataKey: `${sid}_median`, name: legendName, color: v.color, kind: 'line', strokeDasharray: seriesDash, strokeWidth: 2, group: v.group });
      } else {
        series.push({
          dataKey: `s${scIdx}_${v.key}_mean`,
          name: legendName,
          color: v.color,
          kind: 'line',
          strokeDasharray: seriesDash,
          strokeOpacity: 0.85,
          strokeWidth: 2,
          group: v.group,
        });
        for (let r = 0; r < active[scIdx].runs.length; r++) {
          series.push({
            dataKey: `s${scIdx}_${v.key}_r${r}`,
            name: '',
            color: v.color,
            kind: 'line',
            strokeDasharray: seriesDash,
            strokeWidth: 1,
            strokeOpacity: 0.2,
            hideLegend: true,
          });
        }
      }
    }
  }

  return { id, label, description, data, series };
}

/**
 * Build a single-variable view (Prevalence, Incidence, Cumulative).
 * Shows PI ribbon or traces per scenario. Color = scenario.
 */
function buildSingleVarView(
  id: string,
  label: string,
  description: string,
  active: Scenario[],
  timeGrid: number[],
  getVal: (traj: TrajectoryJson, snapIdx: number) => number,
  mode: EnsembleMode,
): PlotView {
  const data: ChartPoint[] = timeGrid.map((t, ti) => {
    const pt: ChartPoint = { t };
    for (const [si, sc] of active.entries()) {
      const values = sc.runs.map((r) => getVal(r.trajectory, ti));
      if (mode === 'pi') {
        const stats = computeStats(values);
        pt[`s${si}_lo`] = stats.lo;
        pt[`s${si}_band`] = stats.band;
        pt[`s${si}_median`] = stats.median;
      } else {
        pt[`s${si}_mean`] =
          values.length > 0 ? values.reduce((a, b) => a + b, 0) / values.length : 0;
        values.forEach((v, ri) => { pt[`s${si}_r${ri}`] = v; });
      }
    }
    return pt;
  });

  const series: ChartSeries[] = [];
  for (const [si, sc] of active.entries()) {
    if (mode === 'pi') {
      const sid = `s${si}`;
      series.push({
        dataKey: `s${si}_lo`,
        name: '',
        color: sc.color,
        kind: 'area_base',
        stackId: sid,
        hideLegend: true,
      });
      series.push({
        dataKey: `s${si}_band`,
        name: '',
        color: sc.color,
        kind: 'area_band',
        stackId: sid,
        fillOpacity: 0.18,
        hideLegend: true,
      });
      series.push({
        dataKey: `s${si}_median`,
        name: sc.name,
        color: sc.color,
        kind: 'line',
        strokeWidth: 2,
      });
    } else {
      series.push({
        dataKey: `s${si}_mean`,
        name: sc.name,
        color: sc.color,
        kind: 'line',
        strokeWidth: 2,
      });
      for (let r = 0; r < sc.runs.length; r++) {
        series.push({
          dataKey: `s${si}_r${r}`,
          name: '',
          color: sc.color,
          kind: 'line',
          strokeWidth: 1,
          strokeOpacity: 0.25,
          hideLegend: true,
        });
      }
    }
  }

  return { id, label, description, data, series };
}

// ── Structural helpers ────────────────────────────────────────────────────────

type DimValues = Record<string, string>;

function cartesianProductDims(dims: Array<{ name: string; values: string[] }>): DimValues[] {
  if (dims.length === 0) return [{}];
  const [first, ...rest] = dims;
  const restCombos = cartesianProductDims(rest);
  return first.values.flatMap(v => restCombos.map(combo => ({ [first.name]: v, ...combo })));
}

/** Map expanded compartment name → dim value assignments for that stratum. */
function buildCompartmentIndex(ms: ModelStructure): Map<string, DimValues> {
  const index = new Map<string, DimValues>();
  for (const base of ms.base_compartments) {
    const dimNames = ms.compartment_dims[base] ?? [];
    const dims = dimNames.map(dn => ms.dimensions.find(d => d.name === dn)!).filter(Boolean);
    for (const combo of cartesianProductDims(dims)) {
      const suffix = dimNames.map(dn => combo[dn]).join('_');
      index.set(suffix ? `${base}_${suffix}` : base, combo);
    }
  }
  return index;
}

// ── Main export ───────────────────────────────────────────────────────────────

export function buildViews(ir: IrModel, scenarios: Scenario[], mode: EnsembleMode): PlotView[] {
  const active = scenarios.filter((s) => s.runs.length > 0);
  if (active.length === 0) return [];

  const refTraj = active[0].runs[0].trajectory;
  const timeGrid = refTraj.snapshots.map((s) => s.t);

  const hasFlows =
    refTraj.transition_names.length > 0 && (refTraj.snapshots[0]?.flows.length ?? 0) > 0;

  const views: PlotView[] = [];

  const ms = ir.model_structure;

  if (!ms) {
    // ── Fallback: no model_structure — flat single view ───────────────────────
    const compNames = [...refTraj.int_compartment_names, ...refTraj.real_compartment_names];
    const vars = compNames.map((name) => {
      const uIdx = name.indexOf('_');
      const base = uIdx > 0 ? name.slice(0, uIdx) : name;
      return {
        key: name.replace(/[^a-zA-Z0-9]/g, '_'),
        label: name,
        color: compartmentColor(base),
        group: base,
        getVal: makeCachedSumGetter([name]),
      };
    });
    views.push(buildMultiVarView(
      'all', 'All', 'All compartments',
      active, timeGrid, vars, mode,
    ));
  } else {
    const compIndex = buildCompartmentIndex(ms);

    // ── 1. Aggregate ─────────────────────────────────────────────────────────
    {
      const vars = ms.base_compartments.map((base) => {
        const members = [...compIndex.entries()]
          .filter(([name]) => name === base || name.startsWith(base + '_'))
          .map(([name]) => name);
        return {
          key: base,
          label: base,
          color: compartmentColor(base),
          group: base,
          getVal: makeCachedSumGetter(members),
        };
      });
      views.push(buildMultiVarView(
        'aggregate', 'Compartments', 'Base compartments, strata summed',
        active, timeGrid, vars, mode,
      ));
    }

    // ── 2. By dimension ──────────────────────────────────────────────────────
    // Skip dimensions with too many values — charts with >20 strata are unreadable
    // and the O(strata²) computation would freeze the UI (e.g. 238-patch models).
    const MAX_DIM_VIEW_VALUES = 20;
    for (const dim of ms.dimensions.filter((d) => d.values.length <= MAX_DIM_VIEW_VALUES)) {
      const vars: Parameters<typeof buildMultiVarView>[5] = [];
      for (const base of ms.base_compartments) {
        const hasDim = (ms.compartment_dims[base] ?? []).includes(dim.name);
        if (!hasDim) {
          // Compartment doesn't vary on this dim — include aggregated
          const members = [...compIndex.entries()]
            .filter(([name]) => name === base || name.startsWith(base + '_'))
            .map(([name]) => name);
          vars.push({
            key: base,
            label: base,
            color: compartmentColor(base),
            group: base,
            getVal: makeCachedSumGetter(members),
          });
        } else {
          for (const [dimValIdx, dimVal] of dim.values.entries()) {
            const members = [...compIndex.entries()]
              .filter(([name, dimMap]) => {
                if (name !== base && !name.startsWith(base + '_')) return false;
                return dimMap[dim.name] === dimVal;
              })
              .map(([name]) => name);
            vars.push({
              key: `${base}_${dimVal}`.replace(/[^a-zA-Z0-9]/g, '_'),
              label: `${base} [${dimVal}]`,
              color: compartmentColor(base),
              group: base,
              strokeDasharray: SCENARIO_DASHES[dimValIdx % SCENARIO_DASHES.length] || undefined,
              getVal: makeCachedSumGetter(members),
            });
          }
        }
      }
      views.push(buildMultiVarView(
        `by_${dim.name}`, `By ${dim.name}`,
        `Compartments by ${dim.name}, colour = type, dash = stratum`,
        active, timeGrid, vars, mode,
      ));
    }

    // ── 3. Prevalence ────────────────────────────────────────────────────────
    if (ms.infectious_compartments.length > 0) {
      const infectiousNames = ms.infectious_compartments.flatMap((base) =>
        [...compIndex.entries()]
          .filter(([name]) => name === base || name.startsWith(base + '_'))
          .map(([name]) => name)
      );
      if (infectiousNames.length > 0) {
        views.push(buildSingleVarView(
          'prevalence', 'Prevalence', 'Infectious compartments (summed)',
          active, timeGrid,
          makeCachedSumGetter(infectiousNames),
          mode,
        ));
      }
    }

    // ── 4 & 5. Incidence + Cumulative ────────────────────────────────────────
    if (hasFlows && ms.transmission_transitions.length > 0) {
      const infTrans = refTraj.transition_names.filter((n) =>
        ms.transmission_transitions.some((base) => n === base || n.startsWith(base + '_'))
      );
      if (infTrans.length > 0) {
        views.push(buildSingleVarView(
          'incidence', 'Incidence', 'New infection events per output interval',
          active, timeGrid,
          (traj, si) => infTrans.reduce((sum, t) => sum + flowAtIdx(traj, si, t), 0),
          mode,
        ));

        // Cumulative
        const cumData: ChartPoint[] = timeGrid.map((t, ti) => {
          const pt: ChartPoint = { t };
          for (const [scIdx, sc] of active.entries()) {
            const values = sc.runs.map((r) => {
              let cumSum = 0;
              for (let j = 0; j <= ti; j++) {
                cumSum += infTrans.reduce((s, tr) => s + flowAtIdx(r.trajectory, j, tr), 0);
              }
              return cumSum;
            });
            if (mode === 'pi') {
              const stats = computeStats(values);
              pt[`s${scIdx}_lo`] = stats.lo;
              pt[`s${scIdx}_band`] = stats.band;
              pt[`s${scIdx}_median`] = stats.median;
            } else {
              pt[`s${scIdx}_mean`] =
                values.length > 0 ? values.reduce((a, b) => a + b, 0) / values.length : 0;
              values.forEach((v, ri) => { pt[`s${scIdx}_r${ri}`] = v; });
            }
          }
          return pt;
        });

        const cumSeries: ChartSeries[] = [];
        for (const [si, sc] of active.entries()) {
          if (mode === 'pi') {
            const sid = `cs${si}`;
            cumSeries.push({ dataKey: `s${si}_lo`, name: '', color: sc.color, kind: 'area_base', stackId: sid, hideLegend: true });
            cumSeries.push({ dataKey: `s${si}_band`, name: '', color: sc.color, kind: 'area_band', stackId: sid, fillOpacity: 0.18, hideLegend: true });
            cumSeries.push({ dataKey: `s${si}_median`, name: sc.name, color: sc.color, kind: 'line', strokeWidth: 2 });
          } else {
            cumSeries.push({ dataKey: `s${si}_mean`, name: sc.name, color: sc.color, kind: 'line', strokeWidth: 2 });
            for (let r = 0; r < sc.runs.length; r++) {
              cumSeries.push({ dataKey: `s${si}_r${r}`, name: '', color: sc.color, kind: 'line', strokeWidth: 1, strokeOpacity: 0.25, hideLegend: true });
            }
          }
        }
        views.push({
          id: 'cumulative',
          label: 'Cumulative',
          description: 'Running total of infection events — final value is epidemic size',
          data: cumData,
          series: cumSeries,
        });
      }
    }
  }

  // ── Flows (always shown if available) ────────────────────────────────────────
  if (hasFlows && refTraj.transition_names.length > 0) {
    const shown = refTraj.transition_names.slice(0, 14);
    const palette = ['#f59e0b', '#3b82f6', '#22c55e', '#a78bfa', '#f97316', '#06b6d4', '#ec4899', '#84cc16'];
    const vars = shown.map((name, i) => {
      const uIdx = name.indexOf('_');
      const transitionBase = uIdx > 0 ? name.slice(0, uIdx) : name;
      return {
        key: `flow_${name.replace(/[^a-zA-Z0-9]/g, '_')}`,
        label: name,
        color: palette[i % palette.length],
        group: transitionBase,
        getVal: (traj: TrajectoryJson, si: number) => flowAtIdx(traj, si, name),
      };
    });
    views.push(buildMultiVarView(
      'flows', 'Flows', 'All transition event counts per output interval',
      active, timeGrid, vars, mode,
    ));
  }

  return views;
}
