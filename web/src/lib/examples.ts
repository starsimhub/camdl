import { parse } from 'smol-toml';

// DSL source — canonical golden files
import sirBasicDsl          from '../../../ocaml/golden/sir_basic.camdl?raw';
import sirDemographyDsl     from '../../../ocaml/golden/sir_demography.camdl?raw';
import seirAgeDsl           from '../../../ocaml/golden/seir_age.camdl?raw';
import seirErlangDsl        from '../../../ocaml/golden/seir_erlang.camdl?raw';
import seirErlangStagedDsl  from '../../../ocaml/golden/seir_erlang_staged.camdl?raw';
import sirFiveAgeDsl        from '../../../ocaml/golden/sir_five_age.camdl?raw';
import sirCouplingDsl       from '../../../ocaml/golden/sir_coupling.camdl?raw';

// Preset files — individual flat TOML per named parameter set
// Baseline files also carry model_label, model_description, and [descriptions]
import sirBasicBaseline          from '../../../ocaml/golden/defaults/sir_basic-baseline.toml?raw';
import sirBasicHighR0            from '../../../ocaml/golden/defaults/sir_basic-high_r0.toml?raw';
import sirDemographyBaseline     from '../../../ocaml/golden/defaults/sir_demography-baseline.toml?raw';
import sirDemographyEndemic      from '../../../ocaml/golden/defaults/sir_demography-endemic.toml?raw';
import seirAgeBaseline           from '../../../ocaml/golden/defaults/seir_age-baseline.toml?raw';
import seirAgeChildDriven        from '../../../ocaml/golden/defaults/seir_age-child_driven.toml?raw';
import seirErlangBaseline        from '../../../ocaml/golden/defaults/seir_erlang-baseline.toml?raw';
import seirErlangSlowProgression from '../../../ocaml/golden/defaults/seir_erlang-slow_progression.toml?raw';
import seirErlangStagedBaseline  from '../../../ocaml/golden/defaults/seir_erlang_staged-baseline.toml?raw';
import sirFiveAgeBaseline        from '../../../ocaml/golden/defaults/sir_five_age-baseline.toml?raw';
import sirFiveAgeSlow            from '../../../ocaml/golden/defaults/sir_five_age-slow.toml?raw';
import sirCouplingBaseline       from '../../../ocaml/golden/defaults/sir_coupling-baseline.toml?raw';
import sirCouplingHighCoupling   from '../../../ocaml/golden/defaults/sir_coupling-high_coupling.toml?raw';

export interface ParamSet {
  name: string;
  label: string;
  values: Record<string, number>;
  tEnd?: number;
}

export interface Example {
  name: string;
  label: string;
  description: string;
  dsl: string;
  /** Named parameter sets; first entry is the default/baseline */
  paramSets: ParamSet[];
  /** Parameter descriptions keyed by param name, from baseline [descriptions] table */
  paramComments: Record<string, string>;
}

/** Parse a single flat preset TOML into a ParamSet. */
function parsePreset(name: string, tomlStr: string): ParamSet {
  const data = parse(tomlStr) as Record<string, unknown>;
  const label = typeof data.label === 'string' ? data.label : name;
  const tEnd  = typeof data.t_end === 'number' ? data.t_end : undefined;
  const values = Object.fromEntries(
    Object.entries(data).filter(([k, v]) =>
      typeof v === 'number' && k !== 't_end'
    ).map(([k, v]) => [k, v as number])
  );
  return { name, label, values, tEnd };
}

/**
 * Build an Example from a DSL string and ordered [name, tomlStr] preset pairs.
 * The first pair must be the baseline — it also carries model_label,
 * model_description, and [descriptions] for param comments.
 */
function buildExample(
  name: string,
  dsl: string,
  ...presets: [presetName: string, tomlStr: string][]
): Example {
  const baseData = parse(presets[0][1]) as Record<string, unknown>;
  const label        = typeof baseData.model_label       === 'string' ? baseData.model_label       : name;
  const description  = typeof baseData.model_description === 'string' ? baseData.model_description : '';
  const paramComments = (baseData.descriptions ?? {}) as Record<string, string>;
  const paramSets = presets.map(([pName, pToml]) => parsePreset(pName, pToml));
  return { name, label, description, dsl, paramSets, paramComments };
}

export const EXAMPLES: Example[] = [
  buildExample('sir_basic',
    sirBasicDsl,
    ['baseline',  sirBasicBaseline],
    ['high_r0',   sirBasicHighR0],
  ),
  buildExample('sir_demography',
    sirDemographyDsl,
    ['baseline',  sirDemographyBaseline],
    ['endemic',   sirDemographyEndemic],
  ),
  buildExample('seir_erlang',
    seirErlangDsl,
    ['baseline',          seirErlangBaseline],
    ['slow_progression',  seirErlangSlowProgression],
  ),
  buildExample('seir_erlang_staged',
    seirErlangStagedDsl,
    ['baseline',  seirErlangStagedBaseline],
  ),
  buildExample('seir_age',
    seirAgeDsl,
    ['baseline',      seirAgeBaseline],
    ['child_driven',  seirAgeChildDriven],
  ),
  buildExample('sir_five_age',
    sirFiveAgeDsl,
    ['baseline',  sirFiveAgeBaseline],
    ['slow',      sirFiveAgeSlow],
  ),
  buildExample('sir_coupling',
    sirCouplingDsl,
    ['baseline',       sirCouplingBaseline],
    ['high_coupling',  sirCouplingHighCoupling],
  ),
];
