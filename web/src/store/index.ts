import { create } from 'zustand';
import type { Node, Edge } from '@xyflow/react';
import type { IrModel, Diagnostic } from '../types/ir';
import type { TrajectoryJson } from '../types/trajectory';
import type { SimConfig } from '../lib/wasm';
import type { Span, SpanMap } from '../lib/spanExtractor';
import { compile as compileApi } from '../lib/compilerClient';
import { simulate as wasmSimulate } from '../lib/wasm';
import { irToCanvas } from '../lib/irToCanvas';
import { extractSpans } from '../lib/spanExtractor';

export type ActiveTab = 'agent' | 'run' | 'split';

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

export interface Scenario {
  id: string;
  name: string;
  /** Full param values (snapshot of baseline + any edits). */
  params: Record<string, number>;
  tEnd?: number;
  seed: number;
  replicates: number;
  trajectories: TrajectoryJson[];
  status: 'idle' | 'running' | 'ok' | 'error';
  error?: string;
}

interface CamdlStore {
  // ── DSL ──────────────────────────────────────────────────────────────────────
  dslSource: string;
  modelName: string;
  setDslSource: (s: string) => void;
  setModelName: (n: string) => void;

  // ── Compilation ───────────────────────────────────────────────────────────────
  ir: IrModel | null;
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

  // ── Simulation ────────────────────────────────────────────────────────────────
  trajectory: TrajectoryJson | null;
  simStatus: 'idle' | 'running' | 'ok' | 'error';
  simError: string | null;
  simConfig: SimConfig;
  setSimConfig: (c: Partial<SimConfig>) => void;
  paramOverrides: Record<string, number>;
  setParamOverride: (name: string, value: number) => void;
  resetParamOverrides: () => void;
  runSimulation: () => Promise<void>;

  // ── Bottom panel ──────────────────────────────────────────────────────────────
  activeTab: ActiveTab;
  setActiveTab: (t: ActiveTab) => void;

  // ── Agent ─────────────────────────────────────────────────────────────────────
  messages: AgentMessage[];
  agentStatus: 'idle' | 'streaming';
  pendingDiff: ProposedEdit | null;
  addUserMessage: (text: string) => void;
  appendAssistantChunk: (id: string, chunk: string) => void;
  startAssistantMessage: (id: string) => void;
  addToolCall: (msgId: string, name: string, status: 'running' | 'done' | 'error', summary: string) => void;
  setAgentStatus: (s: 'idle' | 'streaming') => void;
  setPendingDiff: (d: ProposedEdit | null) => void;
  acceptDiff: () => void;
  rejectDiff: () => void;

  // ── Scenarios ─────────────────────────────────────────────────────────────────
  scenarios: Scenario[];
  addScenario: () => void;
  removeScenario: (id: string) => void;
  updateScenario: (id: string, patch: Partial<Pick<Scenario, 'name' | 'seed' | 'replicates' | 'tEnd'>>) => void;
  updateScenarioParam: (id: string, paramName: string, value: number) => void;
  runScenario: (id: string) => Promise<void>;

  // ── File I/O ──────────────────────────────────────────────────────────────────
  openFile: () => Promise<void>;
  saveFile: () => void;

  // ── Helpers ───────────────────────────────────────────────────────────────────
  /** Clear run results (call before loading a new model). */
  resetRun: () => void;
}

// Debounce helper
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

export const useStore = create<CamdlStore>((set, get) => ({
  // DSL
  dslSource: DEFAULT_DSL,
  modelName: 'sir_basic',
  setModelName: (n) => set({ modelName: n }),

  setDslSource: (s) => {
    set({ dslSource: s });
    // Debounced compile
    if (compileTimer) clearTimeout(compileTimer);
    compileTimer = setTimeout(() => get().compile(), 600);
  },

  // Compilation
  ir: null,
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
        set({
          ir: result.ir,
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
        diagnostics: [{ severity: 'error', code: 'E000', message: String(e), loc: { file: '', line: 0, col: 0, end_line: 0, end_col: 0 } }],
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
    if (id) {
      const span = get().spanMap.get(id) ?? null;
      set({ highlightedSpan: span });
    } else {
      set({ highlightedSpan: null });
    }
  },

  // Simulation
  trajectory: null,
  simStatus: 'idle',
  simError: null,
  simConfig: { backend: 'gillespie', seed: 42 },
  paramOverrides: {},

  setSimConfig: (c) => set((s) => ({ simConfig: { ...s.simConfig, ...c } })),

  setParamOverride: (name, value) =>
    set((s) => ({ paramOverrides: { ...s.paramOverrides, [name]: value } })),

  resetParamOverrides: () => set({ paramOverrides: {} }),

  runSimulation: async () => {
    const { ir, simConfig, paramOverrides } = get();
    if (!ir) return;
    set({ simStatus: 'running', simError: null });
    try {
      // Patch IR with parameter and time-range overrides before sending to WASM
      const patchedIr = {
        ...ir,
        parameters: ir.parameters.map((p) =>
          paramOverrides[p.name] !== undefined
            ? { ...p, value: paramOverrides[p.name] }
            : p
        ),
        simulation: simConfig.tEnd != null
          ? { ...ir.simulation, t_end: simConfig.tEnd }
          : ir.simulation,
      };
      const irJson = JSON.stringify(patchedIr);
      const traj = await wasmSimulate(irJson, simConfig);
      set({ trajectory: traj, simStatus: 'ok', activeTab: 'run' });
    } catch (e) {
      set({ simStatus: 'error', simError: String(e) });
    }
  },

  // Bottom panel
  activeTab: 'agent',
  setActiveTab: (t) => set({ activeTab: t }),

  // Agent
  messages: [],
  agentStatus: 'idle',
  pendingDiff: null,

  addUserMessage: (text) => {
    const msg: AgentMessage = { id: crypto.randomUUID(), role: 'user', content: text };
    set((s) => ({ messages: [...s.messages, msg], activeTab: 'agent' }));
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

  setAgentStatus: (s) => set({ agentStatus: s }),

  setPendingDiff: (d) => set({ pendingDiff: d }),

  acceptDiff: () => {
    const { pendingDiff } = get();
    if (!pendingDiff) return;
    set({ pendingDiff: null });
    get().setDslSource(pendingDiff.modified);
  },

  rejectDiff: () => set({ pendingDiff: null }),

  // Scenarios
  scenarios: [],

  addScenario: () => {
    const { ir, paramOverrides, simConfig, scenarios } = get();
    if (!ir) return;
    const n = scenarios.length + 1;
    // Snapshot full param values: baseline (Params tab overrides) on top of IR defaults
    const params = Object.fromEntries(
      ir.parameters.map((p) => [p.name, paramOverrides[p.name] ?? p.value])
    );
    const scenario: Scenario = {
      id: crypto.randomUUID(),
      name: `Scenario ${n}`,
      params,
      tEnd: simConfig.tEnd,
      seed: simConfig.seed,
      replicates: 5,
      trajectories: [],
      status: 'idle',
    };
    set((s) => ({ scenarios: [...s.scenarios, scenario] }));
  },

  removeScenario: (id) =>
    set((s) => ({ scenarios: s.scenarios.filter((sc) => sc.id !== id) })),

  updateScenario: (id, patch) =>
    set((s) => ({
      scenarios: s.scenarios.map((sc) => sc.id === id ? { ...sc, ...patch } : sc),
    })),

  updateScenarioParam: (id, paramName, value) =>
    set((s) => ({
      scenarios: s.scenarios.map((sc) =>
        sc.id === id ? { ...sc, params: { ...sc.params, [paramName]: value } } : sc
      ),
    })),

  runScenario: async (id) => {
    const state = get();
    const scenario = state.scenarios.find((sc) => sc.id === id);
    if (!scenario || !state.ir) return;

    set((s) => ({
      scenarios: s.scenarios.map((sc) =>
        sc.id === id ? { ...sc, status: 'running', trajectories: [], error: undefined } : sc
      ),
    }));

    try {
      const patchedIr = {
        ...state.ir,
        parameters: state.ir.parameters.map((p) => ({
          ...p,
          value: scenario.params[p.name] ?? p.value,
        })),
        simulation: scenario.tEnd != null
          ? { ...state.ir.simulation, t_end: scenario.tEnd }
          : state.ir.simulation,
      };
      const irJson = JSON.stringify(patchedIr);

      const trajectories: TrajectoryJson[] = [];
      for (let r = 0; r < scenario.replicates; r++) {
        const traj = await wasmSimulate(irJson, { backend: 'gillespie', seed: scenario.seed + r });
        trajectories.push(traj);
      }

      set((s) => ({
        scenarios: s.scenarios.map((sc) =>
          sc.id === id ? { ...sc, status: 'ok', trajectories } : sc
        ),
      }));
    } catch (e) {
      set((s) => ({
        scenarios: s.scenarios.map((sc) =>
          sc.id === id ? { ...sc, status: 'error', error: String(e), trajectories: [] } : sc
        ),
      }));
    }
  },

  // File I/O
  openFile: async () => {
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = '.camdl,.json';
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      const text = await file.text();
      const name = file.name.replace(/\.(camdl|json)$/, '');
      // Reset everything that is model-specific before loading
      get().resetRun();
      get().resetParamOverrides();
      set({ modelName: name, simConfig: { backend: 'gillespie', seed: 42 } });
      get().setDslSource(text);
    };
    input.click();
  },

  resetRun: () => set({ trajectory: null, simStatus: 'idle', simError: null }),

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
}));
