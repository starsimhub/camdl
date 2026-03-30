import type { Diagnostic, IrModel } from "../types/ir";

const BASE = "/api";

export interface CompileSuccess {
  ok: true;
  ir: IrModel;
}

export interface CompileFailure {
  ok: false;
  diagnostics: Diagnostic[];
}

export type CompileResult = CompileSuccess | CompileFailure;

export async function compile(source: string, name?: string): Promise<CompileResult> {
  const res = await fetch(`${BASE}/compile`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ source, name }),
  });
  return res.json();
}
