import type { TrajectoryJson } from '../types/trajectory';
import { compartmentColor } from './irToCanvas';

export interface ChartSeries {
  name: string;
  color: string;
  dataKey: string;
}

export interface ChartPoint {
  t: number;
  [key: string]: number;
}

export function trajectoryToChart(traj: TrajectoryJson): {
  data: ChartPoint[];
  series: ChartSeries[];
} {
  const allNames = [...traj.int_compartment_names, ...traj.real_compartment_names];

  const series: ChartSeries[] = allNames.map((name) => ({
    name,
    color: compartmentColor(name),
    dataKey: name,
  }));

  const data: ChartPoint[] = traj.snapshots.map((snap) => {
    const point: ChartPoint = { t: snap.t };
    traj.int_compartment_names.forEach((name, i) => { point[name] = snap.counts[i] ?? 0; });
    traj.real_compartment_names.forEach((name, i) => { point[name] = snap.values[i] ?? 0; });
    return point;
  });

  return { data, series };
}
