import type { IrModel } from '../types/ir';
import type { TrajectoryJson } from '../types/trajectory';
import type { Scenario } from '../store';
import { compartmentColor } from './irToCanvas';

export interface ChartSeries {
  dataKey: string;
  name: string;
  color: string;
  strokeDasharray?: string;
  strokeOpacity?: number;
  strokeWidth?: number;
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

// ── Stratification detection ──────────────────────────────────────────────────

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

// ── Data builders ─────────────────────────────────────────────────────────────

function rawValue(traj: TrajectoryJson, snapIdx: number, compIdx: number): number {
  const snap = traj.snapshots[snapIdx];
  const nInt = traj.int_compartment_names.length;
  return compIdx < nInt
    ? (snap.counts[compIdx] ?? 0)
    : (snap.values[compIdx - nInt] ?? 0);
}

function allCompNames(traj: TrajectoryJson): string[] {
  return [...traj.int_compartment_names, ...traj.real_compartment_names];
}

/** Build chart data including only the specified keys (plus t). */
function buildData(traj: TrajectoryJson, keys: string[]): ChartPoint[] {
  const names = allCompNames(traj);
  const indices = keys.map((k) => names.indexOf(k));
  return traj.snapshots.map((snap, si) => {
    const pt: ChartPoint = { t: snap.t };
    keys.forEach((k, ki) => {
      if (indices[ki] >= 0) pt[k] = rawValue(traj, si, indices[ki]);
    });
    return pt;
  });
}

/** Build chart data where each key is the SUM of several source compartments. */
function buildAggData(
  traj: TrajectoryJson,
  groups: { key: string; members: string[] }[]
): ChartPoint[] {
  const names = allCompNames(traj);
  return traj.snapshots.map((snap, si) => {
    const pt: ChartPoint = { t: snap.t };
    for (const { key, members } of groups) {
      let sum = 0;
      for (const m of members) {
        const idx = names.indexOf(m);
        if (idx >= 0) sum += rawValue(traj, si, idx);
      }
      pt[key] = sum;
    }
    return pt;
  });
}

/** Build flow (incidence) data from transition counts. */
function buildFlowData(traj: TrajectoryJson): ChartPoint[] {
  return traj.snapshots.map((snap) => {
    const pt: ChartPoint = { t: snap.t };
    traj.transition_names.forEach((name, i) => {
      pt[name] = snap.flows[i] ?? 0;
    });
    return pt;
  });
}

// Dash patterns for distinguishing strata with the same color
const DASHES = ['', '6 3', '2 3', '10 4 2 4', '1 4'];

// ── Main export ───────────────────────────────────────────────────────────────

export function buildViews(ir: IrModel, traj: TrajectoryJson): PlotView[] {
  const names = allCompNames(traj);
  const parsed = parseNames(names);

  const allStrata = uniqueOrdered(
    parsed.filter((p) => p.stratum !== null).map((p) => p.stratum!)
  );
  const isStratified = allStrata.length > 0;

  // All unique base compartment names (respects model order)
  const bases = uniqueOrdered(parsed.map((p) => p.base));

  const views: PlotView[] = [];

  // ── 1. Aggregate view (stratified models only, shown first) ──────────────────
  if (isStratified) {
    const groups = bases.map((base) => ({
      key: base,
      members: parsed.filter((p) => p.base === base).map((p) => p.full),
    }));
    const series: ChartSeries[] = bases.map((base) => ({
      dataKey: base,
      name: base,
      color: compartmentColor(base),
    }));
    views.push({
      id: 'aggregate',
      label: 'Aggregate',
      description: 'Strata summed per compartment type',
      data: buildAggData(traj, groups),
      series,
    });
  }

  // ── 2. All compartments ──────────────────────────────────────────────────────
  {
    const series: ChartSeries[] = parsed.map((p, i) => {
      const stratumIdx = p.stratum ? allStrata.indexOf(p.stratum) : 0;
      return {
        dataKey: p.full,
        name: p.full,
        color: compartmentColor(p.base),
        strokeDasharray: DASHES[stratumIdx % DASHES.length] || undefined,
        strokeOpacity: isStratified ? 0.85 : 1,
      };
    });
    views.push({
      id: 'all',
      label: 'All',
      description: 'All compartments',
      data: buildData(traj, names),
      series,
    });
  }

  // ── 3. By group (stratified only) ────────────────────────────────────────────
  if (isStratified && allStrata.length <= 8) {
    // For each stratum, one series per base compartment within it —
    // colours by compartment type, shown overlaid on one chart
    const series: ChartSeries[] = [];
    for (const [si, stratum] of allStrata.entries()) {
      const members = parsed.filter((p) => p.stratum === stratum);
      for (const p of members) {
        series.push({
          dataKey: p.full,
          name: `${p.base} [${stratum}]`,
          color: compartmentColor(p.base),
          strokeDasharray: DASHES[si % DASHES.length] || undefined,
          strokeOpacity: 0.9,
        });
      }
    }
    views.push({
      id: 'by_group',
      label: 'By group',
      description: 'Compartments by stratum, colour = type, dash = stratum',
      data: buildData(traj, names),
      series,
    });
  }

  // ── 4. Prevalence — infectious compartments only ──────────────────────────────
  const infectiousAll = parsed.filter((p) => /^I/i.test(p.base));
  if (infectiousAll.length > 0) {
    const series: ChartSeries[] = [];
    if (infectiousAll.length > 1 && isStratified) {
      // Add aggregate I_total line first
      series.push({ dataKey: '__I_total', name: 'I (total)', color: '#ef4444' });
    }
    for (const [si, p] of infectiousAll.entries()) {
      series.push({
        dataKey: p.full,
        name: p.stratum ? `I [${p.stratum}]` : 'I',
        color: compartmentColor(p.base),
        strokeDasharray: DASHES[si % DASHES.length] || undefined,
        strokeOpacity: 0.75,
      });
    }
    // Build data with optional __I_total
    const iGroups = infectiousAll.map((p) => p.full);
    const base = buildData(traj, iGroups);
    const data: ChartPoint[] = base.map((pt) => ({
      ...pt,
      __I_total: iGroups.reduce((s, k) => s + (pt[k] ?? 0), 0),
    }));
    views.push({
      id: 'prevalence',
      label: 'Prevalence',
      description: 'Infectious compartments',
      data,
      series,
    });
  }

  // ── 5. Incidence — infection transitions, stratified ─────────────────────────
  const hasFlows = traj.transition_names.length > 0 && traj.snapshots[0]?.flows.length > 0;
  if (hasFlows) {
    const infectionTr = traj.transition_names.filter((n) =>
      /^infection|^force|^incidence|^transmission/i.test(n)
    );
    if (infectionTr.length > 0) {
      const parsedTr = parseNames(infectionTr);
      const trStrata = uniqueOrdered(
        parsedTr.filter((p) => p.stratum !== null).map((p) => p.stratum!)
      );
      const isStratifiedTr = trStrata.length > 0;

      // Stratum colour palette (distinct from compartment colours)
      const stratumPalette = ['#f97316', '#06b6d4', '#a78bfa', '#22c55e', '#f59e0b', '#ec4899'];

      const series: ChartSeries[] = [];

      if (isStratifiedTr) {
        // Aggregate total line first
        series.push({
          dataKey: '__incidence_total',
          name: 'total',
          color: '#e5e7eb',
          strokeOpacity: 0.9,
        });
        // Per-stratum lines, colour by stratum
        for (const [si, p] of parsedTr.entries()) {
          series.push({
            dataKey: p.full,
            name: p.stratum ?? p.full,
            color: stratumPalette[si % stratumPalette.length],
            strokeDasharray: DASHES[si % DASHES.length] || undefined,
            strokeOpacity: 0.85,
          });
        }
      } else {
        series.push({ dataKey: infectionTr[0], name: 'incidence', color: '#f97316' });
      }

      const flowBase = buildFlowData(traj);
      const data: ChartPoint[] = flowBase.map((pt) => ({
        ...pt,
        __incidence_total: infectionTr.reduce((s, k) => s + (pt[k] ?? 0), 0),
      }));

      views.push({
        id: 'incidence',
        label: 'Incidence',
        description: 'New infection events per output interval',
        data,
        series,
      });
    }
  }

  // ── 6. Cumulative incidence ───────────────────────────────────────────────────
  if (hasFlows) {
    const infectionTr = traj.transition_names.filter((n) =>
      /^infection|^force|^incidence|^transmission/i.test(n)
    );
    const sourceTr = infectionTr.length > 0 ? infectionTr : [];
    if (sourceTr.length > 0) {
      const parsedTr = parseNames(sourceTr);
      const trStrata = uniqueOrdered(
        parsedTr.filter((p) => p.stratum !== null).map((p) => p.stratum!)
      );
      const isStratifiedTr = trStrata.length > 0;
      const stratumPalette = ['#f97316', '#06b6d4', '#a78bfa', '#22c55e', '#f59e0b', '#ec4899'];

      // Build cumulative data by running-summing the flows
      const flowBase = buildFlowData(traj);
      const cumSums: Record<string, number> = {};
      const data: ChartPoint[] = flowBase.map((pt) => {
        const out: ChartPoint = { t: pt.t };
        for (const name of sourceTr) {
          cumSums[name] = (cumSums[name] ?? 0) + (pt[name] ?? 0);
          out[`cum_${name}`] = cumSums[name];
        }
        const total = sourceTr.reduce((s, k) => s + (cumSums[k] ?? 0), 0);
        out['__cum_total'] = total;
        return out;
      });

      const series: ChartSeries[] = [];
      if (isStratifiedTr) {
        series.push({ dataKey: '__cum_total', name: 'total', color: '#e5e7eb', strokeOpacity: 0.9 });
        for (const [si, p] of parsedTr.entries()) {
          series.push({
            dataKey: `cum_${p.full}`,
            name: p.stratum ?? p.full,
            color: stratumPalette[si % stratumPalette.length],
            strokeDasharray: DASHES[si % DASHES.length] || undefined,
            strokeOpacity: 0.85,
          });
        }
      } else {
        series.push({ dataKey: `cum_${sourceTr[0]}`, name: 'cumulative infections', color: '#f97316' });
      }

      views.push({
        id: 'cumulative',
        label: 'Cumulative',
        description: 'Running total of infection events — final value is epidemic size',
        data,
        series,
      });
    }
  }

  // ── 7. All flows ──────────────────────────────────────────────────────────────
  if (hasFlows && traj.transition_names.length > 0) {
    const shown = traj.transition_names.slice(0, 14);
    const palette = ['#f59e0b', '#3b82f6', '#22c55e', '#a78bfa', '#f97316', '#06b6d4', '#ec4899', '#84cc16'];
    const series: ChartSeries[] = shown.map((name, i) => ({
      dataKey: name,
      name,
      color: palette[i % palette.length],
      strokeDasharray: i >= palette.length ? DASHES[(Math.floor(i / palette.length)) % DASHES.length] || undefined : undefined,
      strokeOpacity: 0.8,
    }));
    views.push({
      id: 'flows',
      label: 'Flows',
      description: 'All transition event counts per output interval',
      data: buildFlowData(traj),
      series,
    });
  }

  return views;
}

// ── Scenario compare view ─────────────────────────────────────────────────────

const SCENARIO_COLORS = ['#2dd4bf', '#f97316', '#a78bfa', '#22c55e', '#f59e0b', '#ec4899', '#3b82f6', '#f43f5e'];

/** Sum of all I-prefixed integer compartments at a given snapshot index. */
function iTotalAtIdx(traj: TrajectoryJson, snapIdx: number): number {
  const iIndices = traj.int_compartment_names
    .map((n, i) => ({ n, i }))
    .filter(({ n }) => /^I/i.test(n))
    .map(({ i }) => i);
  const snap = traj.snapshots[snapIdx];
  if (iIndices.length === 0) return snap.counts.reduce((a, b) => a + b, 0);
  return iIndices.reduce((sum, i) => sum + (snap.counts[i] ?? 0), 0);
}

/** Forward-fill I_total for a trajectory at time t. */
function iTotalAt(traj: TrajectoryJson, t: number): number {
  let lastIdx = 0;
  for (let i = 0; i < traj.snapshots.length; i++) {
    if (traj.snapshots[i].t <= t + 1e-9) lastIdx = i;
    else break;
  }
  return iTotalAtIdx(traj, lastIdx);
}

/**
 * Build a "Compare" PlotView overlaying infectious prevalence for all
 * completed scenarios. Bold mean + thin individual replicates.
 */
export function buildCompareViews(scenarios: Scenario[]): PlotView[] {
  const ready = scenarios.filter((s) => s.status === 'ok' && s.trajectories.length > 0);
  if (ready.length === 0) return [];

  // Merge time grids from all replicates, downsample to ≤ 400 points
  const rawTimes = [...new Set(
    ready.flatMap((s) => s.trajectories.flatMap((t) => t.snapshots.map((sn) => sn.t)))
  )].sort((a, b) => a - b);

  const MAX = 400;
  const timeGrid = rawTimes.length > MAX
    ? rawTimes.filter((_, i) => i % Math.ceil(rawTimes.length / MAX) === 0)
    : rawTimes;

  const series: ChartSeries[] = [];
  const dataRows: ChartPoint[] = timeGrid.map((t) => ({ t }));

  for (const [si, sc] of ready.entries()) {
    const color = SCENARIO_COLORS[si % SCENARIO_COLORS.length];
    const nRep = sc.trajectories.length;

    // Mean line (bold, shown in legend)
    const meanKey = `s${si}_mean`;
    series.push({ dataKey: meanKey, name: sc.name, color, strokeWidth: 2 });

    // Individual replicate lines (faint, hidden from legend)
    if (nRep > 1) {
      for (let r = 0; r < nRep; r++) {
        series.push({
          dataKey: `s${si}_r${r}`,
          name: `${sc.name} #${r + 1}`,
          color,
          strokeWidth: 1,
          strokeOpacity: 0.25,
          hideLegend: true,
        });
      }
    }

    // Fill data rows
    for (let ti = 0; ti < timeGrid.length; ti++) {
      const t = timeGrid[ti];
      const repVals = sc.trajectories.map((traj) => iTotalAt(traj, t));
      dataRows[ti][meanKey] = repVals.reduce((a, b) => a + b, 0) / repVals.length;
      if (nRep > 1) {
        for (let r = 0; r < nRep; r++) {
          dataRows[ti][`s${si}_r${r}`] = repVals[r];
        }
      }
    }
  }

  return [{
    id: 'compare',
    label: 'Compare',
    description: 'Scenarios overlaid — infectious prevalence (mean + replicates)',
    data: dataRows,
    series,
  }];
}
