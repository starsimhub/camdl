import type { IrModel } from '../types/ir';
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

interface Parsed {
  full: string;
  base: string;
  stratum: string | null;
}

function parseNames(names: string[]): Parsed[] {
  return names.map((full) => {
    const idx = full.indexOf('_');
    return idx > 0
      ? { full, base: full.slice(0, idx), stratum: full.slice(idx + 1) }
      : { full, base: full, stratum: null };
  });
}

function uniqueOrdered<T>(arr: T[]): T[] {
  return [...new Set(arr)];
}

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

function allCompNames(traj: TrajectoryJson): string[] {
  return [...traj.int_compartment_names, ...traj.real_compartment_names];
}

/** Sum named compartments at a snapshot index. */
function sumAtIdx(traj: TrajectoryJson, snapIdx: number, compNames: string[]): number {
  const names = allCompNames(traj);
  const si = Math.min(snapIdx, traj.snapshots.length - 1);
  const snap = traj.snapshots[si];
  if (!snap) return 0;
  const nInt = traj.int_compartment_names.length;
  return compNames.reduce((sum, name) => {
    const idx = names.indexOf(name);
    if (idx < 0) return sum;
    return sum + (idx < nInt ? (snap.counts[idx] ?? 0) : (snap.values[idx - nInt] ?? 0));
  }, 0);
}

/** Get flow value for a named transition at snapshot index. */
function flowAtIdx(traj: TrajectoryJson, snapIdx: number, transName: string): number {
  const si = Math.min(snapIdx, traj.snapshots.length - 1);
  const snap = traj.snapshots[si];
  if (!snap) return 0;
  const idx = traj.transition_names.indexOf(transName);
  return idx >= 0 ? (snap.flows[idx] ?? 0) : 0;
}

// Dash patterns for multi-variable views — one per scenario
const SCENARIO_DASHES = ['', '6 3', '2 3', '10 4 2 4', '1 4'];

/**
 * Build a multi-variable view (Aggregate, All, By Group, Flows).
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
  variables: { key: string; label: string; color: string; strokeDasharray?: string; getVal: (traj: TrajectoryJson, si: number) => number }[],
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
        series.push({ dataKey: `${sid}_median`, name: legendName, color: v.color, kind: 'line', strokeDasharray: seriesDash, strokeWidth: 2 });
      } else {
        series.push({
          dataKey: `s${scIdx}_${v.key}_mean`,
          name: legendName,
          color: v.color,
          kind: 'line',
          strokeDasharray: seriesDash,
          strokeOpacity: 0.85,
          strokeWidth: 2,
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
      // Mean line (bold, in legend)
      series.push({
        dataKey: `s${si}_mean`,
        name: sc.name,
        color: sc.color,
        kind: 'line',
        strokeWidth: 2,
      });
      // Individual trace lines (faint, not in legend)
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

// ── Main export ───────────────────────────────────────────────────────────────

export function buildViews(ir: IrModel, scenarios: Scenario[], mode: EnsembleMode): PlotView[] {
  const active = scenarios.filter((s) => s.runs.length > 0);
  if (active.length === 0) return [];

  // Reference trajectory for compartment names and time grid
  const refTraj = active[0].runs[0].trajectory;
  const compNames = allCompNames(refTraj);
  const parsed = parseNames(compNames);
  const timeGrid = refTraj.snapshots.map((s) => s.t);

  const allStrata = uniqueOrdered(
    parsed.filter((p) => p.stratum !== null).map((p) => p.stratum!)
  );
  const isStratified = allStrata.length > 0;
  const bases = uniqueOrdered(parsed.map((p) => p.base));

  const views: PlotView[] = [];

  // ── 1. Aggregate (stratified only) ───────────────────────────────────────────
  if (isStratified) {
    const vars = bases.map((base) => ({
      key: base,
      label: base,
      color: compartmentColor(base),
      getVal: (traj: TrajectoryJson, si: number) =>
        sumAtIdx(traj, si, parsed.filter((p) => p.base === base).map((p) => p.full)),
    }));
    views.push(buildMultiVarView(
      'aggregate', 'Aggregate', 'Strata summed per compartment type',
      active, timeGrid, vars, mode,
    ));
  }

  // ── 2. All compartments ──────────────────────────────────────────────────────
  {
    const dashesAll = ['', '6 3', '2 3', '10 4 2 4', '1 4'];
    const vars = parsed.map((p, i) => {
      const stratumIdx = p.stratum ? allStrata.indexOf(p.stratum) : 0;
      return {
        key: p.full.replace(/[^a-zA-Z0-9]/g, '_'),
        label: p.full,
        color: compartmentColor(p.base),
        strokeDasharray: dashesAll[stratumIdx % dashesAll.length] || undefined,
        getVal: (traj: TrajectoryJson, si: number) => sumAtIdx(traj, si, [p.full]),
      };
    });
    views.push(buildMultiVarView(
      'all', 'All', 'All compartments',
      active, timeGrid, vars, mode,
    ));
  }

  // ── 3. By group (stratified, ≤ 8 strata) ─────────────────────────────────────
  if (isStratified && allStrata.length <= 8) {
    const dashesGroup = ['', '6 3', '2 3', '10 4 2 4', '1 4'];
    const vars: Parameters<typeof buildMultiVarView>[6] = [];
    for (const [si, stratum] of allStrata.entries()) {
      const members = parsed.filter((p) => p.stratum === stratum);
      for (const p of members) {
        vars.push({
          key: `${p.full.replace(/[^a-zA-Z0-9]/g, '_')}_grp`,
          label: `${p.base} [${stratum}]`,
          color: compartmentColor(p.base),
          strokeDasharray: dashesGroup[si % dashesGroup.length] || undefined,
          getVal: (traj: TrajectoryJson, snapIdx: number) => sumAtIdx(traj, snapIdx, [p.full]),
        });
      }
    }
    views.push(buildMultiVarView(
      'by_group', 'By group', 'Compartments by stratum, colour = type, dash = stratum',
      active, timeGrid, vars, mode,
    ));
  }

  // ── 4. Prevalence (I-prefixed compartments, aggregated) ──────────────────────
  const infectiousNames = parsed.filter((p) => /^I/i.test(p.base)).map((p) => p.full);
  if (infectiousNames.length > 0) {
    views.push(buildSingleVarView(
      'prevalence', 'Prevalence', 'Infectious compartments (summed)',
      active, timeGrid,
      (traj, si) => sumAtIdx(traj, si, infectiousNames),
      mode,
    ));
  }

  // ── 5. Incidence (infection transitions, summed) ─────────────────────────────
  const hasFlows =
    refTraj.transition_names.length > 0 && (refTraj.snapshots[0]?.flows.length ?? 0) > 0;

  if (hasFlows) {
    const infTrans = refTraj.transition_names.filter((n) =>
      /^infection|^force|^incidence|^transmission/i.test(n)
    );
    if (infTrans.length > 0) {
      views.push(buildSingleVarView(
        'incidence', 'Incidence', 'New infection events per output interval',
        active, timeGrid,
        (traj, si) => infTrans.reduce((sum, t) => sum + flowAtIdx(traj, si, t), 0),
        mode,
      ));

      // ── 6. Cumulative incidence ───────────────────────────────────────────────
      // We need running sums — build special view
      const cumData: ChartPoint[] = timeGrid.map((t, ti) => {
        const pt: ChartPoint = { t };
        for (const [scIdx, sc] of active.entries()) {
          const values = sc.runs.map((r) => {
            // Sum flows up to this snapshot index
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

      // Series same structure as buildSingleVarView
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
        supportsEnsembleMode: true,
      });
    }
  }

  // ── 7. All flows ──────────────────────────────────────────────────────────────
  if (hasFlows && refTraj.transition_names.length > 0) {
    const shown = refTraj.transition_names.slice(0, 14);
    const palette = ['#f59e0b', '#3b82f6', '#22c55e', '#a78bfa', '#f97316', '#06b6d4', '#ec4899', '#84cc16'];
    const vars = shown.map((name, i) => ({
      key: `flow_${name.replace(/[^a-zA-Z0-9]/g, '_')}`,
      label: name,
      color: palette[i % palette.length],
      getVal: (traj: TrajectoryJson, si: number) => flowAtIdx(traj, si, name),
    }));
    views.push(buildMultiVarView(
      'flows', 'Flows', 'All transition event counts per output interval',
      active, timeGrid, vars, mode,
    ));
  }

  return views;
}
