import dagre from 'dagre';
import type { Node, Edge } from '@xyflow/react';

const NODE_W = 100;
const NODE_H = 60;

export function applyDagreLayout(nodes: Node[], edges: Edge[]): { nodes: Node[]; edges: Edge[] } {
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: 'LR', ranksep: 80, nodesep: 50, marginx: 40, marginy: 40 });

  for (const node of nodes) {
    g.setNode(node.id, { width: NODE_W, height: NODE_H });
  }
  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  const laidOut = nodes.map((node) => {
    const pos = g.node(node.id);
    return {
      ...node,
      position: { x: pos.x - NODE_W / 2, y: pos.y - NODE_H / 2 },
    };
  });

  return { nodes: laidOut, edges };
}

/**
 * Grid layout for stratified models.
 * Rows = compartment base type (S, I, R, …) ordered by epidemic role.
 * Columns = strata in declaration order.
 */
const EPIDEMIC_ORDER = ['S', 'E', 'I', 'R', 'D'];
const H_STEP = 170; // column spacing
const V_STEP = 110; // row spacing

export function applyGridLayout(
  nodes: Node[],
  edges: Edge[],
  bases: string[],   // unique base names in desired row order
  strata: string[],  // unique strata in desired column order
): { nodes: Node[]; edges: Edge[] } {
  // Sort bases by epidemic role, then alphabetical for unknowns
  const sortedBases = [...bases].sort((a, b) => {
    const ai = EPIDEMIC_ORDER.findIndex((r) => a.toUpperCase().startsWith(r));
    const bi = EPIDEMIC_ORDER.findIndex((r) => b.toUpperCase().startsWith(r));
    if (ai !== bi) return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
    return a.localeCompare(b);
  });

  const laidOut = nodes.map((node) => {
    // node id is `comp:S_age_0_5` — strip prefix
    const name = node.id.replace(/^comp:/, '');
    const underIdx = name.indexOf('_');
    const base = underIdx > 0 ? name.slice(0, underIdx) : name;
    const stratum = underIdx > 0 ? name.slice(underIdx + 1) : '';

    const col = stratum ? strata.indexOf(stratum) : 0;
    const row = sortedBases.indexOf(base);

    return {
      ...node,
      position: {
        x: (col === -1 ? 0 : col) * H_STEP,
        y: (row === -1 ? 0 : row) * V_STEP,
      },
    };
  });

  return { nodes: laidOut, edges };
}
