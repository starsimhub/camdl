/**
 * Utilities for extracting patch-level statistics from stratified model trajectories.
 *
 * Supports two patch conventions:
 *   1. Numeric suffix: `_p{N}` (e.g. `S_p0`, `I_a1_p3`) — legacy / simple models.
 *   2. Named slug: determined from IrModel.model_structure dimension named "patch"
 *      (e.g. `S_borno_gwoza`, `I_kano_gwale`) — patch-stratified models with named LGAs.
 *
 * "Patch index" is the 0-based position in the patch values list.
 * "Patch name" is the slug for that index (or `p${N}` for numeric convention).
 */

import type { IrModel } from '../types/ir';
import type { TrajectoryJson } from '../types/trajectory';
import type { Scenario } from '../types/experiment';

export interface PatchInfo {
  /** Sorted list of all patch indices (0-based). */
  indices: number[];
  /** Patch name (slug) for each index — used as compartment name suffix. */
  names: string[];
  /** Unique compartment-type prefixes (first `_`-separated token). */
  compTypes: string[];
  /** Number of snapshots in the trajectory. */
  nSnapshots: number;
  /** Time value at each snapshot. */
  tValues: number[];
}

function quantile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0;
  if (sorted.length === 1) return sorted[0];
  const pos = q * (sorted.length - 1);
  const lo = Math.floor(pos), hi = Math.ceil(pos);
  return lo === hi ? sorted[lo] : sorted[lo] * (hi - pos) + sorted[hi] * (pos - lo);
}

/** Detect patch structure from a trajectory, optionally using IR model_structure. */
export function detectPatches(traj: TrajectoryJson, ir?: IrModel | null): PatchInfo | null {
  const allNames = [...traj.int_compartment_names, ...traj.real_compartment_names];

  // ── Strategy 1: use model_structure dimension named "patch" ────────────────
  const patchDim = ir?.model_structure?.dimensions?.find((d) => d.name === 'patch');
  if (patchDim && patchDim.values.length > 0) {
    const slugs = patchDim.values;
    const slugSet = new Set(slugs);

    // Confirm at least one compartment has a matching suffix
    const hasPatch = allNames.some((n) => {
      const uIdx = n.indexOf('_');
      if (uIdx < 0) return false;
      const suffix = n.slice(uIdx + 1);
      return slugSet.has(suffix);
    });
    if (!hasPatch) return null;

    // Extract compartment type prefixes (first `_`-token) for compartments with a patch suffix
    const types = new Set<string>();
    for (const n of allNames) {
      const uIdx = n.indexOf('_');
      if (uIdx < 0) continue;
      const base = n.slice(0, uIdx);
      const suffix = n.slice(uIdx + 1);
      if (slugSet.has(suffix)) types.add(base);
    }

    return {
      indices: slugs.map((_, i) => i),
      names: slugs,
      compTypes: [...types].sort(),
      nSnapshots: traj.snapshots.length,
      tValues: traj.snapshots.map((s) => s.t),
    };
  }

  // ── Strategy 2: legacy `_p{N}` suffix ─────────────────────────────────────
  const numericIndices = new Set<number>();
  for (const n of allNames) {
    const m = n.match(/_p(\d+)$/);
    if (m) numericIndices.add(parseInt(m[1]));
  }
  if (numericIndices.size === 0) return null;

  const types = new Set<string>();
  for (const n of allNames) {
    if (/_p\d+$/.test(n)) {
      types.add(n.split('_')[0]);
    }
  }

  const sorted = [...numericIndices].sort((a, b) => a - b);
  return {
    indices: sorted,
    names: sorted.map((n) => `p${n}`),
    compTypes: [...types].sort(),
    nSnapshots: traj.snapshots.length,
    tValues: traj.snapshots.map((s) => s.t),
  };
}

/** Sum of a given compartment type at a patch and snapshot index (single trajectory). */
export function patchCompSum(
  traj: TrajectoryJson,
  compType: string,
  patchIdx: number,
  snapIdx: number,
  patchNames?: string[],
): number {
  const si = Math.min(snapIdx, traj.snapshots.length - 1);
  const snap = traj.snapshots[si];
  if (!snap) return 0;
  const suffix = `_${patchNames ? patchNames[patchIdx] : `p${patchIdx}`}`;
  let total = 0;
  for (let i = 0; i < traj.int_compartment_names.length; i++) {
    const n = traj.int_compartment_names[i];
    if (n.endsWith(suffix) && n.slice(0, n.length - suffix.length) === compType) {
      total += snap.counts[i] ?? 0;
    }
  }
  for (let i = 0; i < traj.real_compartment_names.length; i++) {
    const n = traj.real_compartment_names[i];
    if (n.endsWith(suffix) && n.slice(0, n.length - suffix.length) === compType) {
      total += snap.values[i] ?? 0;
    }
  }
  return total;
}

/** Median of summed compartment type at a patch across all seeds in a scenario. */
export function medianPatchValue(
  sc: Scenario,
  compType: string,
  patchIdx: number,
  snapIdx: number,
  patchNames?: string[],
): number {
  if (sc.runs.length === 0) return 0;
  const vals = sc.runs
    .map((r) => patchCompSum(r.trajectory, compType, patchIdx, snapIdx, patchNames))
    .sort((a, b) => a - b);
  return quantile(vals, 0.5);
}

/** Compute median values for all patches at a snapshot. Returns array in patchIndices order. */
export function allPatchMedians(
  sc: Scenario,
  patchIndices: number[],
  compType: string,
  snapIdx: number,
  patchNames?: string[],
): number[] {
  return patchIndices.map((p) => medianPatchValue(sc, compType, p, snapIdx, patchNames));
}

/** Per-seed time series for a single patch: returns array of {t, ...seedValues}. */
export function patchTimeSeries(
  sc: Scenario,
  compType: string,
  patchIdx: number,
  patchNames?: string[],
): { t: number; median: number; [key: string]: number }[] {
  if (sc.runs.length === 0) return [];
  const nSnaps = sc.runs[0].trajectory.snapshots.length;
  const result = [];
  for (let si = 0; si < nSnaps; si++) {
    const t = sc.runs[0].trajectory.snapshots[si].t;
    const seedVals = sc.runs.map((r) => patchCompSum(r.trajectory, compType, patchIdx, si, patchNames));
    const sorted = [...seedVals].sort((a, b) => a - b);
    const row: { t: number; median: number; [key: string]: number } = {
      t,
      median: quantile(sorted, 0.5),
    };
    for (let k = 0; k < sc.runs.length; k++) {
      row[`seed_${sc.runs[k].seed}`] = seedVals[k];
    }
    result.push(row);
  }
  return result;
}
