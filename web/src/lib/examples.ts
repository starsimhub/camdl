// DSL source — canonical golden files (imported as raw text for the code editor)
import sirBasicDsl          from '../../../ocaml/golden/sir_basic.camdl?raw';
import sirDemographyDsl     from '../../../ocaml/golden/sir_demography.camdl?raw';
import seirAgeDsl           from '../../../ocaml/golden/seir_age.camdl?raw';
import seirErlangDsl        from '../../../ocaml/golden/seir_erlang.camdl?raw';
import seirErlangStagedDsl  from '../../../ocaml/golden/seir_erlang_staged.camdl?raw';
import sirFiveAgeDsl        from '../../../ocaml/golden/sir_five_age.camdl?raw';
import sirCouplingDsl       from '../../../ocaml/golden/sir_coupling.camdl?raw';

// Compiled IR JSON — carries presets and model metadata
import sirBasicIr          from '../../../ocaml/golden/sir_basic.ir.json';
import sirDemographyIr     from '../../../ocaml/golden/sir_demography.ir.json';
import seirAgeIr           from '../../../ocaml/golden/seir_age.ir.json';
import seirErlangIr        from '../../../ocaml/golden/seir_erlang.ir.json';
import seirErlangStagedIr  from '../../../ocaml/golden/seir_erlang_staged.ir.json';
import sirFiveAgeIr        from '../../../ocaml/golden/sir_five_age.ir.json';
import sirCouplingIr       from '../../../ocaml/golden/sir_coupling.ir.json';

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
}

interface IrPreset {
  name: string;
  label: string;
  params: Record<string, number>;
  t_end?: number | null;
}

interface IrModel {
  name?: string;
  description?: string | null;
  presets?: IrPreset[];
}

/** Build an Example from a DSL string and compiled IR object. */
function buildExample(name: string, dsl: string, ir: IrModel): Example {
  const label       = ir.name ?? name;
  const description = ir.description ?? '';
  const paramSets: ParamSet[] = (ir.presets ?? []).map(p => ({
    name:   p.name,
    label:  p.label,
    values: p.params,
    tEnd:   p.t_end ?? undefined,
  }));
  return { name, label, description, dsl, paramSets };
}

export const EXAMPLES: Example[] = [
  buildExample('sir_basic',          sirBasicDsl,         sirBasicIr as IrModel),
  buildExample('sir_demography',     sirDemographyDsl,    sirDemographyIr as IrModel),
  buildExample('seir_erlang',        seirErlangDsl,       seirErlangIr as IrModel),
  buildExample('seir_erlang_staged', seirErlangStagedDsl, seirErlangStagedIr as IrModel),
  buildExample('seir_age',           seirAgeDsl,          seirAgeIr as IrModel),
  buildExample('sir_five_age',       sirFiveAgeDsl,       sirFiveAgeIr as IrModel),
  buildExample('sir_coupling',       sirCouplingDsl,      sirCouplingIr as IrModel),
];
