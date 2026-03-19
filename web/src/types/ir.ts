// TypeScript mirror of ir/schema.json

export type CompartmentKind = 'integer' | 'real';

export interface Compartment {
  name: string;
  kind: CompartmentKind;
}

export type Expr = Record<string, unknown>; // opaque — we pretty-print it

export interface Transition {
  name: string;
  stoichiometry: [string, number][];
  rate: Expr;
  event_key: string | null;
  metadata: {
    origin_kind?: string;
    source_compartment?: string;
    dest_compartment?: string;
  } | null;
}

export interface Parameter {
  name: string;
  value: number;
  prior: unknown | null;
  transform: string | null;
}

export interface SimulationConfig {
  t_start: number;
  t_end: number;
  time_semantics: string;
  dt: number | null;
  rng_seed: number | null;
}

export interface OutputSchedule {
  regular?: { start: number; step: number; end: number };
  at_times?: number[];
  match_observations?: true;
}

export interface OutputConfig {
  times: OutputSchedule;
  format: string;
  trajectory: boolean;
  observations: boolean;
}

export interface Dimension {
  name: string;
  values: string[];
}

export interface ModelStructure {
  dimensions: Dimension[];
  compartment_dims: Record<string, string[]>;  // base → [dim_name, ...]
  base_compartments: string[];
  transmission_transitions: string[];
  infectious_compartments: string[];
}

export interface IrModel {
  name: string;
  version: string;
  description: string | null;
  compartments: Compartment[];
  transitions: Transition[];
  ode_equations: unknown[];
  time_functions: unknown[];
  tables: unknown[];
  interventions: unknown[];
  observations: unknown[];
  parameters: Parameter[];
  initial_conditions: unknown;
  data_contract: unknown | null;
  output: OutputConfig;
  simulation: SimulationConfig;
  model_structure?: ModelStructure;
}

export interface Diagnostic {
  severity: 'error' | 'warning';
  code: string;
  message: string;
  loc: {
    file: string;
    line: number;
    col: number;
    end_line: number;
    end_col: number;
  };
  detail?: string;
  hint?: string;
}
