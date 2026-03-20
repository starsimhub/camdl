/**
 * Utilities for extracting patch-level statistics from stratified model trajectories.
 *
 * Assumes patch stratification uses the `_p{N}` suffix convention:
 *   - `S_p0`, `I_a1_p3`, `flow_infection_p12`, etc.
 *
 * "Patch index" is the integer N in the last `_p{N}` suffix token.
 * "Compartment type" is the first `_`-separated token before any stratification
 *   suffixes, e.g. `I_a2_p5` → type = `I`.
 */

import type { TrajectoryJson } from '../types/trajectory';
import type { Scenario } from '../types/experiment';

export interface PatchInfo {
  /** Sorted list of all patch indices found in the trajectory. */
  indices: number[];
  /** Unique compartment-type prefixes (first token before first `_a` or `_p`). */
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

/** Detect patch structure from a trajectory. Returns null if no patches found. */
export function detectPatches(traj: TrajectoryJson): PatchInfo | null {
  const allNames = [...traj.int_compartment_names, ...traj.real_compartment_names];
  const indices = new Set<number>();
  for (const n of allNames) {
    const m = n.match(/_p(\d+)$/);
    if (m) indices.add(parseInt(m[1]));
  }
  if (indices.size === 0) return null;

  // Extract unique compartment type prefixes (first `_`-token)
  const types = new Set<string>();
  for (const n of allNames) {
    if (/_p\d+$/.test(n)) {
      types.add(n.split('_')[0]);
    }
  }

  return {
    indices: [...indices].sort((a, b) => a - b),
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
): number {
  const si = Math.min(snapIdx, traj.snapshots.length - 1);
  const snap = traj.snapshots[si];
  if (!snap) return 0;
  const suffix = `_p${patchIdx}`;
  let total = 0;
  for (let i = 0; i < traj.int_compartment_names.length; i++) {
    const n = traj.int_compartment_names[i];
    if (n.endsWith(suffix) && n.split('_')[0] === compType) {
      total += snap.counts[i] ?? 0;
    }
  }
  for (let i = 0; i < traj.real_compartment_names.length; i++) {
    const n = traj.real_compartment_names[i];
    if (n.endsWith(suffix) && n.split('_')[0] === compType) {
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
): number {
  if (sc.runs.length === 0) return 0;
  const vals = sc.runs
    .map((r) => patchCompSum(r.trajectory, compType, patchIdx, snapIdx))
    .sort((a, b) => a - b);
  return quantile(vals, 0.5);
}

/** Compute median values for all patches at a snapshot. Returns array in patchIndices order. */
export function allPatchMedians(
  sc: Scenario,
  patchIndices: number[],
  compType: string,
  snapIdx: number,
): number[] {
  return patchIndices.map((p) => medianPatchValue(sc, compType, p, snapIdx));
}

/** Per-seed time series for a single patch: returns array of {t, ...seedValues}. */
export function patchTimeSeries(
  sc: Scenario,
  compType: string,
  patchIdx: number,
): { t: number; median: number; [key: string]: number }[] {
  if (sc.runs.length === 0) return [];
  const nSnaps = sc.runs[0].trajectory.snapshots.length;
  const result = [];
  for (let si = 0; si < nSnaps; si++) {
    const t = sc.runs[0].trajectory.snapshots[si].t;
    const seedVals = sc.runs.map((r) => patchCompSum(r.trajectory, compType, patchIdx, si));
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
