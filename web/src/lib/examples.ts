// Auto-discover all golden models from ocaml/golden/*.camdl + *.ir.json.
// Any new golden file pair is picked up automatically — no manual registration needed.

const dslFiles = import.meta.glob("../../../ocaml/golden/*.camdl", {
  query: "?raw",
  import: "default",
  eager: true,
}) as Record<string, string>;

const irFiles = import.meta.glob("../../../ocaml/golden/*.ir.json", {
  eager: true,
}) as Record<string, { default: IrModel }>;

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
  const label = ir.name ?? name;
  const description = ir.description ?? "";
  const paramSets: ParamSet[] = (ir.scenarios ?? ir.presets ?? []).map(p => ({
    name: p.name,
    label: p.label,
    values: p.params,
    tEnd: p.t_end ?? undefined,
  }));
  return { name, label, description, dsl, paramSets };
}

function stemOf(path: string): string {
  return path.replace(/.*\//, "").replace(/\.[^.]+$/, "");
}

export const EXAMPLES: Example[] = Object.entries(dslFiles)
  .map(([dslPath, dsl]) => {
    const name = stemOf(dslPath);
    const irPath = dslPath.replace(/\.camdl$/, ".ir.json");
    const irMod = irFiles[irPath];
    if (!irMod) return null;
    return buildExample(name, dsl, irMod.default as IrModel);
  })
  .filter((e): e is Example => e !== null)
  .sort((a, b) => a.name.localeCompare(b.name));
