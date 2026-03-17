import type { Node, Edge } from '@xyflow/react';
import type { IrModel, Expr } from '../types/ir';
import { applyDagreLayout } from './canvasLayout';

// ── Expression pretty-printer ─────────────────────────────────────────────────

function ppExpr(e: Expr): string {
  if (!e || typeof e !== 'object') return String(e);
  const keys = Object.keys(e);
  if (keys.length === 0) return '?';

  if ('const' in e) return String((e as { const: { value: number } }).const.value ?? e.const);
  if ('param' in e) return String((e as { param: string }).param);
  if ('pop' in e) return String((e as { pop: string }).pop);
  if ('pop_sum' in e) {
    const names = (e as { pop_sum: string[] }).pop_sum;
    return `(${names.join('+')})`;
  }
  if ('time' in e) return 't';
  if ('time_func' in e) return String((e as { time_func: { name: string } }).time_func.name);
  if ('table_lookup' in e) {
    const tl = (e as { table_lookup: { table: string } }).table_lookup;
    return `${tl.table}[…]`;
  }
  if ('bin_op' in e) {
    const bo = (e as { bin_op: { op: string; left: Expr; right: Expr } }).bin_op;
    const opStr: Record<string, string> = {
      add: '+', sub: '-', mul: '·', div: '/', pow: '^', min: 'min', max: 'max',
      eq: '=', neq: '≠', lt: '<', gt: '>', le: '≤', ge: '≥',
    };
    const op = opStr[bo.op] ?? bo.op;
    return `${ppExpr(bo.left)}${op}${ppExpr(bo.right)}`;
  }
  if ('un_op' in e) {
    const uo = (e as { un_op: { op: string; arg: Expr } }).un_op;
    return `${uo.op}(${ppExpr(uo.arg)})`;
  }
  if ('cond' in e) {
    const c = (e as { cond: { pred: Expr; then: Expr; else_: Expr } }).cond;
    return `if ${ppExpr(c.pred)} then …`;
  }
  return keys[0];
}

// Truncate long rate strings for edge labels
function truncate(s: string, max = 28) {
  return s.length > max ? s.slice(0, max - 1) + '…' : s;
}

// ── Compartment colour by epidemic role ──────────────────────────────────────

const COMP_COLORS: [RegExp, string][] = [
  [/^S/i, '#3b82f6'],  // blue — susceptible
  [/^E/i, '#f59e0b'],  // amber — exposed
  [/^I/i, '#ef4444'],  // red — infectious
  [/^R/i, '#22c55e'],  // green — recovered/removed
  [/^D/i, '#6b7280'],  // gray — dead
  [/^W/i, '#a78bfa'],  // purple — environmental/water
  [/^V/i, '#06b6d4'],  // cyan — vaccinated
];

export function compartmentColor(name: string): string {
  for (const [re, color] of COMP_COLORS) {
    if (re.test(name)) return color;
  }
  return '#8b5cf6'; // purple fallback
}

// ── IR → React Flow ───────────────────────────────────────────────────────────

export function irToCanvas(model: IrModel): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = model.compartments.map((c) => ({
    id: `comp:${c.name}`,
    type: 'compartmentNode',
    data: { label: c.name, kind: c.kind, color: compartmentColor(c.name) },
    position: { x: 0, y: 0 }, // dagre will set this
  }));

  const edges: Edge[] = model.transitions.map((tr, i) => {
    // Derive source/target from stoichiometry or metadata
    const meta = tr.metadata;
    let source = meta?.source_compartment;
    let target = meta?.dest_compartment;

    if (!source || !target) {
      const neg = tr.stoichiometry.filter(([, d]) => d < 0).map(([n]) => n);
      const pos = tr.stoichiometry.filter(([, d]) => d > 0).map(([n]) => n);
      source = neg[0] ?? pos[0];
      target = pos[0] ?? neg[0];
    }

    const rateStr = truncate(ppExpr(tr.rate));

    return {
      id: `tr:${tr.name}:${i}`,
      source: `comp:${source}`,
      target: `comp:${target}`,
      type: 'transitionEdge',
      data: { label: tr.name, rate: rateStr, originKind: meta?.origin_kind ?? 'intrinsic' },
      markerEnd: { type: 'arrowclosed' as const },
    };
  });

  return applyDagreLayout(nodes, edges);
}
