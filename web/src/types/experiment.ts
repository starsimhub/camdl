import type { TrajectoryJson } from './trajectory';

export interface RunConfig {
  backend: 'gillespie' | 'tau_leap' | 'chain_binomial';
  nSeeds: number;
  baseSeed: number;
  tEnd?: number;
  dt?: number;
}

export interface ScenarioRun {
  seed: number;
  trajectory: TrajectoryJson;
}

export interface Scenario {
  id: string;
  name: string;
  color: string;
  /** Diff-only param overrides relative to IR defaults. Empty = baseline. */
  paramOverrides: Record<string, number>;
  runs: ScenarioRun[];
  seedsCompleted: number;
  status: 'idle' | 'running' | 'ok' | 'error';
  error?: string;
}
