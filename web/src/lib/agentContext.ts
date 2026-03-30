import type { RunConfig, Scenario } from "../types/experiment";
import type { IrModel } from "../types/ir";
import type { TrajectoryJson } from "../types/trajectory";

/** Compute mean of the final snapshot across runs for each compartment. */
function meanFinalState(runs: { trajectory: TrajectoryJson }[]): Record<string, number> | null {
  if (runs.length === 0) return null;

  const result: Record<string, number> = {};
  for (const { trajectory: traj } of runs) {
    const last = traj.snapshots[traj.snapshots.length - 1];
    if (!last) continue;
    traj.int_compartment_names.forEach((name, i) => {
      result[name] = (result[name] ?? 0) + (last.counts[i] ?? 0);
    });
    traj.real_compartment_names.forEach((name, i) => {
      result[name] = (result[name] ?? 0) + (last.values[i] ?? 0);
    });
  }
  for (const key of Object.keys(result)) {
    result[key] = Math.round((result[key] / runs.length) * 100) / 100;
  }
  return result;
}

/** Build a structured context object to inject into every agent turn. */
export function buildAgentContext(
  ir: IrModel | null,
  compileStatus: string,
  scenarios: Scenario[],
  runConfig: RunConfig,
  experimentStatus: string,
): object {
  const ctx: Record<string, unknown> = {
    compile_status: compileStatus,
    experiment_status: experimentStatus,
    run_config: {
      backend: runConfig.backend,
      n_seeds: runConfig.nSeeds,
      base_seed: runConfig.baseSeed,
      ...(runConfig.tEnd != null ? { t_end: runConfig.tEnd } : {}),
      ...(runConfig.dt != null ? { dt: runConfig.dt } : {}),
    },
  };

  if (ir) {
    ctx.model = {
      name: ir.name,
      description: ir.description ?? null,
      time_unit: ir.simulation.time_semantics,
      simulation: {
        t_start: ir.simulation.t_start,
        t_end: ir.simulation.t_end,
      },
      compartments: ir.compartments.map((c) => ({ name: c.name, kind: c.kind })),
      parameters: ir.parameters.map((p) => ({
        name: p.name,
        value: p.value,
        ...(p.transform ? { transform: p.transform } : {}),
      })),
      ...(ir.model_structure?.dimensions?.length
        ? { dimensions: ir.model_structure.dimensions }
        : {}),
    };
  }

  ctx.scenarios = scenarios.map((sc, idx) => {
    const entry: Record<string, unknown> = {
      name: idx === 0 ? "Baseline" : sc.name,
      is_baseline: idx === 0,
      status: sc.status,
      seeds_completed: sc.seedsCompleted,
      param_overrides: sc.paramOverrides,
    };
    if (sc.runs.length > 0) {
      entry.final_state_mean = meanFinalState(sc.runs);
    }
    if (sc.error) entry.error = sc.error;
    return entry;
  });

  return ctx;
}
