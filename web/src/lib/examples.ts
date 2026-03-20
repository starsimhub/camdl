// DSL source — canonical golden files (imported as raw text for the code editor)
import sirBasicDsl              from '../../../ocaml/golden/sir_basic.camdl?raw';
import sirDemographyDsl         from '../../../ocaml/golden/sir_demography.camdl?raw';
import seirAgeDsl               from '../../../ocaml/golden/seir_age.camdl?raw';
import seirErlangDsl            from '../../../ocaml/golden/seir_erlang.camdl?raw';
import seirErlangStagedDsl      from '../../../ocaml/golden/seir_erlang_staged.camdl?raw';
import sirFiveAgeDsl            from '../../../ocaml/golden/sir_five_age.camdl?raw';
import sirCouplingDsl           from '../../../ocaml/golden/sir_coupling.camdl?raw';
import malariaTwoSpeciesDsl     from '../../../ocaml/golden/malaria_two_species.camdl?raw';
import seirVaccineDsl           from '../../../ocaml/golden/seir_vaccine.camdl?raw';
import seirVaccineSeasonalDsl   from '../../../ocaml/golden/seir_vaccine_seasonal.camdl?raw';
import polioAgeDsl              from '../../../ocaml/golden/polio_age.camdl?raw';
import polioSpatial5Dsl         from '../../../ocaml/golden/polio_spatial_5.camdl?raw';
import sirPatches5Dsl           from '../../../ocaml/golden/sir_patches_5.camdl?raw';

// Compiled IR JSON — carries presets and model metadata
import sirBasicIr              from '../../../ocaml/golden/sir_basic.ir.json';
import sirDemographyIr         from '../../../ocaml/golden/sir_demography.ir.json';
import seirAgeIr               from '../../../ocaml/golden/seir_age.ir.json';
import seirErlangIr            from '../../../ocaml/golden/seir_erlang.ir.json';
import seirErlangStagedIr      from '../../../ocaml/golden/seir_erlang_staged.ir.json';
import sirFiveAgeIr            from '../../../ocaml/golden/sir_five_age.ir.json';
import sirCouplingIr           from '../../../ocaml/golden/sir_coupling.ir.json';
import malariaTwoSpeciesIr     from '../../../ocaml/golden/malaria_two_species.ir.json';
import seirVaccineIr           from '../../../ocaml/golden/seir_vaccine.ir.json';
import seirVaccineSeasonalIr   from '../../../ocaml/golden/seir_vaccine_seasonal.ir.json';
import polioAgeIr              from '../../../ocaml/golden/polio_age.ir.json';
import polioSpatial5Ir         from '../../../ocaml/golden/polio_spatial_5.ir.json';
import sirPatches5Ir           from '../../../ocaml/golden/sir_patches_5.ir.json';

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
  scenarios?: IrPreset[];
  /** @deprecated use scenarios */
  presets?: IrPreset[];
}

/** Build an Example from a DSL string and compiled IR object. */
function buildExample(name: string, dsl: string, ir: IrModel): Example {
  const label       = ir.name ?? name;
  const description = ir.description ?? '';
  const paramSets: ParamSet[] = (ir.scenarios ?? ir.presets ?? []).map(p => ({
    name:   p.name,
    label:  p.label,
    values: p.params,
    tEnd:   p.t_end ?? undefined,
  }));
  return { name, label, description, dsl, paramSets };
}

export const EXAMPLES: Example[] = [
  buildExample('sir_basic',              sirBasicDsl,            sirBasicIr as IrModel),
  buildExample('sir_demography',         sirDemographyDsl,       sirDemographyIr as IrModel),
  buildExample('seir_erlang',            seirErlangDsl,          seirErlangIr as IrModel),
  buildExample('seir_erlang_staged',     seirErlangStagedDsl,    seirErlangStagedIr as IrModel),
  buildExample('seir_age',              seirAgeDsl,              seirAgeIr as IrModel),
  buildExample('sir_five_age',          sirFiveAgeDsl,           sirFiveAgeIr as IrModel),
  buildExample('sir_coupling',          sirCouplingDsl,          sirCouplingIr as IrModel),
  buildExample('malaria_two_species',   malariaTwoSpeciesDsl,    malariaTwoSpeciesIr as IrModel),
  buildExample('seir_vaccine',          seirVaccineDsl,          seirVaccineIr as IrModel),
  buildExample('seir_vaccine_seasonal', seirVaccineSeasonalDsl,  seirVaccineSeasonalIr as IrModel),
  buildExample('polio_age',             polioAgeDsl,             polioAgeIr as IrModel),
  buildExample('polio_spatial_5',       polioSpatial5Dsl,        polioSpatial5Ir as IrModel),
  buildExample('sir_patches_5',         sirPatches5Dsl,          sirPatches5Ir as IrModel),
];
