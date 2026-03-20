import type { IrModel } from '../types/ir';
import type { Scenario, ScenarioRun } from '../types/experiment';
import type { TrajectoryJson, TrajectorySnapshot } from '../types/trajectory';

// Mirror of SCENARIO_COLORS from store — avoids circular import
const SCENARIO_COLORS = [
  '#2dd4bf', '#f97316', '#a78bfa', '#22c55e',
  '#f59e0b', '#ec4899', '#3b82f6', '#f43f5e',
];

interface RemoteRunEntry {
  scenario: string;
  seed: number;
  input_hash: string;
}

interface RemoteManifest {
  model: string;
  scenarios: string[];
  seeds: number[];
  total_runs: number;
  completed: number;
  output_dir: string;
  runs: RemoteRunEntry[];
}

export interface RemoteExperiment {
  ir: IrModel;
  scenarios: Scenario[];
}

/** Parse a TSV trajectory into TrajectoryJson using the IR for column mapping. */
function parseTsvTrajectory(tsv: string, ir: IrModel): TrajectoryJson {
  const lines = tsv.trim().split('\n');
  if (lines.length < 1) throw new Error('empty trajectory TSV');
  const headers = lines[0].split('\t');

  const intNames = ir.compartments
    .filter((c) => c.kind === 'integer')
    .map((c) => c.name);
  const realNames = ir.compartments
    .filter((c) => c.kind === 'real')
    .map((c) => c.name);
  const trNames = ir.transitions.map((t) => t.name);

  const tIdx = 0;
  const intIdxs = intNames.map((n) => headers.indexOf(n));
  const realIdxs = realNames.map((n) => headers.indexOf(n));
  const flowIdxs = trNames.map((n) => headers.indexOf('flow_' + n));

  const snapshots: TrajectorySnapshot[] = lines
    .slice(1)
    .filter((l) => l.trim() !== '')
    .map((line) => {
      const cols = line.split('\t').map(Number);
      return {
        t: cols[tIdx] ?? 0,
        counts: intIdxs.map((i) => (i >= 0 ? (cols[i] ?? 0) : 0)),
        values: realIdxs.map((i) => (i >= 0 ? (cols[i] ?? 0) : 0)),
        flows: flowIdxs.map((i) => (i >= 0 ? (cols[i] ?? 0) : 0)),
      };
    });

  return {
    int_compartment_names: intNames,
    real_compartment_names: realNames,
    transition_names: trNames,
    snapshots,
  };
}

/**
 * Fetch an experiment from a running `camdl serve` server and construct Scenario[].
 *
 * Protocol:
 *   GET {baseUrl}/manifest.json  — scenario names, seed list, completed run entries
 *   GET {baseUrl}/model.ir.json  — full IR for compartment/transition mapping
 *   GET {baseUrl}/runs/{hash}/traj.tsv — one per completed run (fetched in parallel)
 */
export async function loadRemoteExperiment(baseUrl: string): Promise<RemoteExperiment> {
  const url = baseUrl.replace(/\/$/, '');

  // 1. Fetch manifest
  const manifestRes = await fetch(`${url}/manifest.json`);
  if (!manifestRes.ok) {
    throw new Error(
      `Could not fetch manifest.json (${manifestRes.status} ${manifestRes.statusText}). ` +
      `Is camdl serve running at ${url}?`
    );
  }
  const manifest: RemoteManifest = await manifestRes.json();

  if (!manifest.runs || manifest.runs.length === 0) {
    throw new Error(
      `No completed runs found in manifest (completed=${manifest.completed ?? 0}/${manifest.total_runs ?? 0}). ` +
      `Run "camdl experiment run" first.`
    );
  }

  // 2. Fetch IR model
  const irRes = await fetch(`${url}/model.ir.json`);
  if (!irRes.ok) {
    throw new Error(
      `Could not fetch model.ir.json (${irRes.status} ${irRes.statusText}). ` +
      `Re-run the experiment to regenerate it.`
    );
  }
  const ir: IrModel = await irRes.json();

  // 3. Fetch all trajectories in parallel
  const loadedRuns = await Promise.all(
    manifest.runs.map(async (run) => {
      const tsvRes = await fetch(`${url}/runs/${run.input_hash}/traj.tsv`);
      if (!tsvRes.ok) {
        throw new Error(`Could not fetch trajectory for run ${run.input_hash} (${tsvRes.status})`);
      }
      const tsv = await tsvRes.text();
      const trajectory = parseTsvTrajectory(tsv, ir);
      return { run, trajectory };
    })
  );

  // 4. Group by scenario name → build Scenario[]
  const grouped = new Map<string, ScenarioRun[]>();
  for (const { run, trajectory } of loadedRuns) {
    if (!grouped.has(run.scenario)) grouped.set(run.scenario, []);
    grouped.get(run.scenario)!.push({ seed: run.seed, trajectory });
  }

  const scenarios: Scenario[] = manifest.scenarios.map((name, i) => ({
    id: crypto.randomUUID(),
    name: i === 0 ? 'Baseline' : name,
    color: SCENARIO_COLORS[i % SCENARIO_COLORS.length],
    paramOverrides: {},
    runs: grouped.get(name) ?? [],
    seedsCompleted: grouped.get(name)?.length ?? 0,
    status: 'ok' as const,
  }));

  return { ir, scenarios };
}
