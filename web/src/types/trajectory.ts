export interface TrajectorySnapshot {
  t: number;
  counts: number[];  // integer compartments in model order
  values: number[];  // real compartments in model order
  flows: number[];   // one per transition
}

export interface TrajectoryJson {
  int_compartment_names: string[];
  real_compartment_names: string[];
  transition_names: string[];
  snapshots: TrajectorySnapshot[];
}
