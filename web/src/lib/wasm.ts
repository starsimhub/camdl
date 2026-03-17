import type { TrajectoryJson } from '../types/trajectory';

export interface SimConfig {
  backend: 'gillespie' | 'tau_leap' | 'chain_binomial';
  seed: number;
  dt?: number;
  output_dt?: number;
  tEnd?: number;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let mod: any = null;
let loading: Promise<void> | null = null;

async function load() {
  if (mod) return;
  if (!loading) {
    loading = import('./wasm/pkg/camdl_wasm.js').then(async (m) => {
      await m.default(); // wasm init
      mod = m;
    });
  }
  await loading;
}

export async function validateIr(ir_json: string): Promise<{ ok: boolean; error?: string }> {
  await load();
  return JSON.parse(mod.validate(ir_json));
}

export async function simulate(ir_json: string, config: SimConfig): Promise<TrajectoryJson> {
  await load();
  const result = JSON.parse(mod.simulate(ir_json, JSON.stringify(config)));
  if (result.error) throw new Error(result.error);
  return result as TrajectoryJson;
}
