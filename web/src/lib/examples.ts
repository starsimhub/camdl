import { parse } from 'smol-toml';

// DSL source — canonical golden files
import sirBasicDsl       from '../../../ocaml/golden/sir_basic.camdl?raw';
import sirDemographyDsl  from '../../../ocaml/golden/sir_demography.camdl?raw';
import seirAgeDsl        from '../../../ocaml/golden/seir_age.camdl?raw';
import seirErlangDsl     from '../../../ocaml/golden/seir_erlang.camdl?raw';
import sirFiveAgeDsl     from '../../../ocaml/golden/sir_five_age.camdl?raw';
import sirCouplingDsl    from '../../../ocaml/golden/sir_coupling.camdl?raw';

// Metadata + parameter sets — canonical TOML files
import sirBasicToml      from '../../../ocaml/golden/defaults/sir_basic.toml?raw';
import sirDemographyToml from '../../../ocaml/golden/defaults/sir_demography.toml?raw';
import seirAgeToml       from '../../../ocaml/golden/defaults/seir_age.toml?raw';
import seirErlangToml    from '../../../ocaml/golden/defaults/seir_erlang.toml?raw';
import sirFiveAgeToml    from '../../../ocaml/golden/defaults/sir_five_age.toml?raw';
import sirCouplingToml   from '../../../ocaml/golden/defaults/sir_coupling.toml?raw';

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
  /** Named parameter sets; first entry is the default */
  paramSets: ParamSet[];
  /** Inline TOML comments keyed by parameter name, e.g. { beta: "transmission rate (day⁻¹)" } */
  paramComments: Record<string, string>;
}

interface TomlParams {
  name: string;
  label: string;
  [key: string]: unknown;
}

interface TomlFile {
  label: string;
  description: string;
  descriptions?: Record<string, string>;
  params: TomlParams[];
}

function fromFiles(name: string, dsl: string, tomlStr: string): Example {
  const data = parse(tomlStr) as unknown as TomlFile;
  const paramComments = data.descriptions ?? {};
  const paramSets: ParamSet[] = (data.params ?? []).map((p) => {
    const { name: pName, label: pLabel, ...rest } = p;
    const tEnd = typeof p.t_end === 'number' ? (p.t_end as number) : undefined;
    const values = Object.fromEntries(
      Object.entries(rest)
        .filter(([k, v]) => typeof v === 'number' && k !== 't_end')
        .map(([k, v]) => [k, v as number])
    );
    return { name: pName, label: pLabel, values, tEnd };
  });
  return { name, label: data.label, description: data.description, dsl, paramSets, paramComments };
}

export const EXAMPLES: Example[] = [
  fromFiles('sir_basic',      sirBasicDsl,      sirBasicToml),
  fromFiles('sir_demography', sirDemographyDsl, sirDemographyToml),
  fromFiles('seir_age',       seirAgeDsl,       seirAgeToml),
  fromFiles('seir_erlang',    seirErlangDsl,    seirErlangToml),
  fromFiles('sir_five_age',   sirFiveAgeDsl,    sirFiveAgeToml),
  fromFiles('sir_coupling',   sirCouplingDsl,   sirCouplingToml),
];
