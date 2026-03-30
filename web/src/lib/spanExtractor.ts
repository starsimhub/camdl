// Lightweight JS span extractor for camdl DSL.
// Identifies line/col ranges for named declarations so the canvas can
// highlight them in the Monaco editor when clicked.

export interface Span {
  startLine: number; // 1-indexed
  startCol: number;
  endLine: number;
  endCol: number;
}

export type SpanMap = Map<string, Span>; // "comp:S" | "tr:infection" | "param:beta" → Span

export function extractSpans(source: string): SpanMap {
  const map: SpanMap = new Map();
  const lines = source.split("\n");

  // Track which block we're in
  let inCompartments = false;
  let inTransitions = false;
  let inParameters = false;

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const lineNo = i + 1;
    const trimmed = line.trim();

    // Detect block starts
    if (/^compartments\s*\{/.test(trimmed)) {
      inCompartments = true;
      inTransitions = false;
      inParameters = false;
    } else if (/^transitions\s*\{/.test(trimmed)) {
      inTransitions = true;
      inCompartments = false;
      inParameters = false;
    } else if (/^parameters\s*\{/.test(trimmed)) {
      inParameters = true;
      inCompartments = false;
      inTransitions = false;
    } else if (/^\}/.test(trimmed)) {
      inCompartments = false;
      inTransitions = false;
      inParameters = false;
    }

    // Compartments block: comma-separated identifiers
    if (inCompartments) {
      const names = trimmed.replace(/[{}]/g, "").split(/[\s,]+/).filter(n => /^[A-Za-z_]\w*$/.test(n));
      for (const name of names) {
        if (!map.has(`comp:${name}`)) {
          const col = line.indexOf(name) + 1;
          map.set(`comp:${name}`, { startLine: lineNo, startCol: col, endLine: lineNo, endCol: col + name.length });
        }
      }
    }

    // Transitions block: "name : ..." or "name[...] : ..."
    if (inTransitions) {
      const m = trimmed.match(/^([A-Za-z_]\w*)(?:\[[^\]]*\])?\s*:/);
      if (m) {
        const name = m[1];
        const col = line.indexOf(name) + 1;
        if (!map.has(`tr:${name}`)) {
          map.set(`tr:${name}`, { startLine: lineNo, startCol: col, endLine: lineNo, endCol: col + name.length });
        }
      }
    }

    // Parameters block: "name : type" or "name = value"
    if (inParameters) {
      const m = trimmed.match(/^([A-Za-z_]\w*)\s*[:=]/);
      if (m) {
        const name = m[1];
        const col = line.indexOf(name) + 1;
        if (!map.has(`param:${name}`)) {
          map.set(`param:${name}`, { startLine: lineNo, startCol: col, endLine: lineNo, endCol: col + name.length });
        }
      }
    }
  }

  return map;
}
