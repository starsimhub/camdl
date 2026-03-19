import { create } from 'zustand';
import type { Node, Edge } from '@xyflow/react';
import type { IrModel, Diagnostic } from '../types/ir';
import type { RunConfig, Scenario } from '../types/experiment';
import type { Span, SpanMap } from '../lib/spanExtractor';
import { compile as compileApi } from '../lib/compilerClient';
import { simulate as wasmSimulate } from '../lib/wasm';
import { irToCanvas } from '../lib/irToCanvas';
import { extractSpans } from '../lib/spanExtractor';
import { EXAMPLES } from '../lib/examples';

export type ActiveTab = 'dsl' | 'agent';

export interface AgentMessage {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  toolCalls?: { name: string; status: 'running' | 'done' | 'error'; summary: string }[];
}

export interface ProposedEdit {
  modified: string;
  explanation: string;
}

// Re-export for consumers
export type { Scenario, RunConfig, ScenarioRun } from '../types/experiment';

// ── Constants ─────────────────────────────────────────────────────────────────

const SCENARIO_COLORS = [
  '#2dd4bf', '#f97316', '#a78bfa', '#22c55e',
  '#f59e0b', '#ec4899', '#3b82f6', '#f43f5e',
];

const SCENARIO_DASHES = ['', '6 3', '2 3', '10 4 2 4', '1 4'];

function nextColor(scenarios: Scenario[]): string {
  return SCENARIO_COLORS[scenarios.length % SCENARIO_COLORS.length];
}

function nextDash(scenarios: Scenario[]): string {
  return SCENARIO_DASHES[scenarios.length % SCENARIO_DASHES.length];
}

export { SCENARIO_COLORS, SCENARIO_DASHES, nextDash };

function makeBaseline(): Scenario {
  return {
    id: crypto.randomUUID(),
    name: 'Baseline',
    color: SCENARIO_COLORS[0],
    paramOverrides: {},
    runs: [],
    seedsCompleted: 0,
    status: 'idle',
  };
}

/** Build a scenario list from IR presets (pure). */
function scenariosFromPresets(presets: IrModel['presets']): Scenario[] {
  if (!presets || presets.length === 0) return [makeBaseline()];
  return presets.map((p, i) => ({
    id: crypto.randomUUID(),
    name: i === 0 ? 'Baseline' : p.label,
    color: SCENARIO_COLORS[i % SCENARIO_COLORS.length],
    paramOverrides: { ...p.params },
    runs: [],
    seedsCompleted: 0,
    status: 'idle' as const,
  }));
}

/** Apply param overrides + runConfig.tEnd to an IR model (pure). */
function patchIr(ir: IrModel, paramOverrides: Record<string, number>, runConfig: RunConfig): IrModel {
  return {
    ...ir,
    parameters: ir.parameters.map((p) =>
      paramOverrides[p.name] !== undefined ? { ...p, value: paramOverrides[p.name] } : p
    ),
    simulation:
      runConfig.tEnd != null ? { ...ir.simulation, t_end: runConfig.tEnd } : ir.simulation,
  };
}

// ── Store interface ────────────────────────────────────────────────────────────

interface CamdlStore {
  // ── DSL ──────────────────────────────────────────────────────────────────────
  dslSource: string;
  modelName: string;
  setDslSource: (s: string) => void;
  setModelName: (n: string) => void;

  // ── Compilation ───────────────────────────────────────────────────────────────
  ir: IrModel | null;
  irHash: string | null;
  diagnostics: Diagnostic[];
  compileStatus: 'idle' | 'compiling' | 'ok' | 'error';
  compile: () => Promise<void>;

  // ── Canvas ────────────────────────────────────────────────────────────────────
  canvasNodes: Node[];
  canvasEdges: Edge[];
  selectedNodeId: string | null;
  spanMap: SpanMap;
  highlightedSpan: Span | null;
  selectNode: (id: string | null) => void;

  // ── Run config ────────────────────────────────────────────────────────────────
  runConfig: RunConfig;
  setRunConfig: (c: Partial<RunConfig>) => void;

  // ── Experiment ────────────────────────────────────────────────────────────────
  experimentStatus: 'idle' | 'running' | 'ok' | 'error';
  runExperiment: () => Promise<void>;
  stopExperiment: () => void;

  // ── Scenarios ─────────────────────────────────────────────────────────────────
  scenarios: Scenario[];
  addScenario: (fromBaseline?: boolean) => void;
  addPresetScenario: (presetName: string) => void;
  removeScenario: (id: string) => void;
  renameScenario: (id: string, name: string) => void;
  setScenarioParam: (id: string, paramName: string, value: number) => void;
  clearScenarioParam: (id: string, paramName: string) => void;

  // ── Bottom panel ──────────────────────────────────────────────────────────────
  activeTab: ActiveTab;
  setActiveTab: (t: ActiveTab) => void;

  // ── Agent ─────────────────────────────────────────────────────────────────────
  messages: AgentMessage[];
  /** idle → waiting → streaming ↔ tool_calling → idle */
  agentPhase: 'idle' | 'waiting' | 'streaming' | 'tool_calling';
  pendingDiff: ProposedEdit | null;
  addUserMessage: (text: string) => void;
  appendAssistantChunk: (id: string, chunk: string) => void;
  startAssistantMessage: (id: string) => void;
  addToolCall: (msgId: string, name: string, status: 'running' | 'done' | 'error', summary: string) => void;
  setAgentPhase: (p: 'idle' | 'waiting' | 'streaming' | 'tool_calling') => void;
  setPendingDiff: (d: ProposedEdit | null) => void;
  acceptDiff: () => void;
  rejectDiff: () => void;

  // ── File I/O ──────────────────────────────────────────────────────────────────
  loadExample: (name: string) => void;
  openFile: () => Promise<void>;
  saveFile: () => void;

  // ── Helpers ───────────────────────────────────────────────────────────────────
  resetExperiment: () => void;
}

// ── Defaults ──────────────────────────────────────────────────────────────────

let compileTimer: ReturnType<typeof setTimeout> | null = null;

const DEFAULT_DSL = `time_unit = 'days

compartments { S, I, R }

let N = S + I + R

parameters {
  beta  : rate
  gamma : rate
  N0    : count
  I0    : count
}

transitions {
  infection : S --> I  @ beta * S * (I / N)
  recovery  : I --> R  @ gamma * I
}

init {
  S = N0 - I0
  I = I0
}

simulate {
  from = 0 'days
  to   = 120 'days
}
`;

// ── Store ─────────────────────────────────────────────────────────────────────

export const useStore = create<CamdlStore>((set, get) => ({
  // DSL
  dslSource: DEFAULT_DSL,
  modelName: 'sir_basic',
  setModelName: (n) => set({ modelName: n }),

  setDslSource: (s) => {
    set({ dslSource: s });
    if (compileTimer) clearTimeout(compileTimer);
    compileTimer = setTimeout(() => get().compile(), 600);
  },

  // Compilation
  ir: null,
  irHash: null,
  diagnostics: [],
  compileStatus: 'idle',

  compile: async () => {
    const { dslSource, modelName } = get();
    set({ compileStatus: 'compiling' });
    try {
      const result = await compileApi(dslSource, modelName);
      if (result.ok) {
        const { nodes, edges } = irToCanvas(result.ir);
        const spanMap = extractSpans(dslSource);
        const newHash = JSON.stringify(result.ir);
        const prevHash = get().irHash;

        const irChanged = newHash !== prevHash;

        if (irChanged) {
          const prevPresetKey = JSON.stringify((get().ir?.presets ?? []).map(p => p.name));
          const newPresetKey  = JSON.stringify((result.ir.presets ?? []).map(p => p.name));
          const presetsChanged = prevPresetKey !== newPresetKey;

          if (presetsChanged) {
            // Presets changed (agent edited scenarios block, or new model loaded) — rebuild scenarios.
            const newScenarios = scenariosFromPresets(result.ir.presets);
            const tEnd = result.ir.presets?.[0]?.t_end ?? undefined;
            set((s) => ({
              scenarios: newScenarios,
              experimentStatus: 'idle',
              runConfig: tEnd != null ? { ...s.runConfig, tEnd } : s.runConfig,
            }));
          } else {
            // IR changed but presets didn't — just clear runs.
            set((s) => ({
              scenarios: s.scenarios.map((sc) => ({
                ...sc, runs: [], seedsCompleted: 0, status: 'idle' as const, error: undefined,
              })),
              experimentStatus: 'idle',
            }));
          }
        }

        set({
          ir: result.ir,
          irHash: newHash,
          diagnostics: [],
          compileStatus: 'ok',
          canvasNodes: nodes,
          canvasEdges: edges,
          spanMap,
        });
      } else {
        set({ ir: null, diagnostics: result.diagnostics, compileStatus: 'error' });
      }
    } catch (e) {
      set({
        compileStatus: 'error',
        diagnostics: [
          {
            severity: 'error',
            code: 'E000',
            message: String(e),
            loc: { file: '', line: 0, col: 0, end_line: 0, end_col: 0 },
          },
        ],
      });
    }
  },

  // Canvas
  canvasNodes: [],
  canvasEdges: [],
  selectedNodeId: null,
  spanMap: new Map(),
  highlightedSpan: null,

  selectNode: (id) => {
    set({ selectedNodeId: id });
    const span = id ? (get().spanMap.get(id) ?? null) : null;
    set({ highlightedSpan: span });
  },

  // Run config
  runConfig: { backend: 'gillespie', nSeeds: 10, baseSeed: 42 },
  setRunConfig: (c) => set((s) => ({ runConfig: { ...s.runConfig, ...c } })),

  // Experiment
  experimentStatus: 'idle',

  runExperiment: async () => {
    const { ir, runConfig, scenarios } = get();
    if (!ir) return;

    // Reset all scenario runs
    set((s) => ({
      experimentStatus: 'running',
      scenarios: s.scenarios.map((sc) => ({
        ...sc,
        runs: [],
        seedsCompleted: 0,
        status: 'idle' as const,
        error: undefined,
      })),
    }));

    const seeds = Array.from({ length: runConfig.nSeeds }, (_, i) => runConfig.baseSeed + i);

    outer: for (const seed of seeds) {
      for (const sc of get().scenarios) {
        // Check stop
        if (get().experimentStatus !== 'running') break outer;

        const currentIr = get().ir;
        if (!currentIr) break outer;

        // Mark this scenario as running
        set((s) => ({
          scenarios: s.scenarios.map((scenario) =>
            scenario.id === sc.id ? { ...scenario, status: 'running' } : scenario
          ),
        }));

        try {
          const patched = patchIr(currentIr, sc.paramOverrides, runConfig);
          const traj = await wasmSimulate(JSON.stringify(patched), {
            backend: runConfig.backend,
            seed,
            dt: runConfig.dt,
          });

          set((s) => ({
            scenarios: s.scenarios.map((scenario) =>
              scenario.id === sc.id
                ? {
                    ...scenario,
                    runs: [...scenario.runs, { seed, trajectory: traj }],
                    seedsCompleted: scenario.seedsCompleted + 1,
                    status: 'running' as const,
                  }
                : scenario
            ),
          }));
        } catch (e) {
          set((s) => ({
            scenarios: s.scenarios.map((scenario) =>
              scenario.id === sc.id
                ? { ...scenario, status: 'error' as const, error: String(e) }
                : scenario
            ),
          }));
        }
      }
    }

    // Finalize
    set((s) => ({
      experimentStatus: s.experimentStatus === 'running' ? 'ok' : s.experimentStatus,
      scenarios: s.scenarios.map((sc) =>
        sc.status === 'running' ? { ...sc, status: 'ok' as const } : sc
      ),
    }));
  },

  stopExperiment: () => {
    set({ experimentStatus: 'ok' });
  },

  // Scenarios
  scenarios: [makeBaseline()],

  addScenario: (fromBaseline = false) => {
    set((s) => {
      const src = fromBaseline ? s.scenarios[0] : s.scenarios[s.scenarios.length - 1];
      const n = s.scenarios.length;
      const scenario: Scenario = {
        id: crypto.randomUUID(),
        name: `Scenario ${n}`,
        color: SCENARIO_COLORS[n % SCENARIO_COLORS.length],
        paramOverrides: src ? { ...src.paramOverrides } : {},
        runs: [],
        seedsCompleted: 0,
        status: 'idle',
      };
      return { scenarios: [...s.scenarios, scenario] };
    });
  },

  addPresetScenario: (presetName) => {
    const { modelName, scenarios } = get();
    const ex = EXAMPLES.find((e) => e.name === modelName);
    const preset = ex?.paramSets.find((p) => p.name === presetName);
    if (!preset) return;
    const n = scenarios.length;
    const scenario: Scenario = {
      id: crypto.randomUUID(),
      name: preset.label,
      color: SCENARIO_COLORS[n % SCENARIO_COLORS.length],
      paramOverrides: { ...preset.values },
      runs: [],
      seedsCompleted: 0,
      status: 'idle',
    };
    set((s) => ({ scenarios: [...s.scenarios, scenario] }));
  },

  removeScenario: (id) =>
    set((s) => ({
      scenarios: s.scenarios.filter((sc) => sc.id !== id),
    })),

  renameScenario: (id, name) =>
    set((s) => ({
      scenarios: s.scenarios.map((sc) => (sc.id === id ? { ...sc, name } : sc)),
    })),

  setScenarioParam: (id, paramName, value) =>
    set((s) => ({
      scenarios: s.scenarios.map((sc) =>
        sc.id === id
          ? { ...sc, paramOverrides: { ...sc.paramOverrides, [paramName]: value } }
          : sc
      ),
    })),

  clearScenarioParam: (id, paramName) =>
    set((s) => ({
      scenarios: s.scenarios.map((sc) => {
        if (sc.id !== id) return sc;
        const { [paramName]: _removed, ...rest } = sc.paramOverrides;
        return { ...sc, paramOverrides: rest };
      }),
    })),

  // Editor panel tab
  activeTab: 'dsl',
  setActiveTab: (t) => set({ activeTab: t }),

  // Agent
  messages: [],
  agentPhase: 'idle',
  pendingDiff: null,

  addUserMessage: (text) => {
    const msg: AgentMessage = { id: crypto.randomUUID(), role: 'user', content: text };
    set((s) => ({ messages: [...s.messages, msg], activeTab: 'agent' as const }));
  },

  startAssistantMessage: (id) => {
    const msg: AgentMessage = { id, role: 'assistant', content: '' };
    set((s) => ({ messages: [...s.messages, msg] }));
  },

  appendAssistantChunk: (id, chunk) => {
    set((s) => ({
      messages: s.messages.map((m) =>
        m.id === id ? { ...m, content: m.content + chunk } : m
      ),
    }));
  },

  addToolCall: (msgId, name, status, summary) => {
    set((s) => ({
      messages: s.messages.map((m) => {
        if (m.id !== msgId) return m;
        const existing = m.toolCalls ?? [];
        const idx = existing.findIndex((tc) => tc.name === name && tc.status === 'running');
        if (idx >= 0) {
          const updated = [...existing];
          updated[idx] = { name, status, summary };
          return { ...m, toolCalls: updated };
        }
        return { ...m, toolCalls: [...existing, { name, status, summary }] };
      }),
    }));
  },

  setAgentPhase: (p) => set({ agentPhase: p }),
  setPendingDiff: (d) => set({ pendingDiff: d }),

  acceptDiff: () => {
    const { pendingDiff } = get();
    if (!pendingDiff) return;
    set({ pendingDiff: null });
    get().setDslSource(pendingDiff.modified);
  },

  rejectDiff: () => set({ pendingDiff: null }),

  // File I/O
  loadExample: (name) => {
    const ex = EXAMPLES.find((e) => e.name === name);
    if (!ex) return;
    // Reset irHash so compile always sees presets as "changed" and rebuilds scenarios.
    set({ modelName: ex.name, irHash: null, scenarios: [makeBaseline()], experimentStatus: 'idle' });
    get().setDslSource(ex.dsl);
  },

  openFile: async () => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.camdl,.json';
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      const text = await file.text();
      const name = file.name.replace(/\.(camdl|json)$/, '');
      get().resetExperiment();
      set({ modelName: name, runConfig: { backend: 'gillespie', nSeeds: 10, baseSeed: 42 } });
      get().setDslSource(text);
    };
    input.click();
  },

  saveFile: () => {
    const { dslSource, modelName } = get();
    const blob = new Blob([dslSource], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `${modelName}.camdl`;
    a.click();
    URL.revokeObjectURL(url);
  },

  resetExperiment: () =>
    set((s) => ({
      experimentStatus: 'idle',
      // Clear runs AND paramOverrides — called on loadExample/openFile where
      // the model changes entirely, making old overrides meaningless.
      scenarios: s.scenarios.map((sc) => ({
        ...sc,
        runs: [],
        seedsCompleted: 0,
        status: 'idle' as const,
        error: undefined,
        paramOverrides: {},
      })),
    })),
}));
