import { NodeType } from 'ui';

/** Material Symbols icon of each kind of node. */
export const NODE_ICONS: Record<NodeType, string> = {
  portfolio: 'cases',
  program: 'hub',
  project: 'folder',
};

/**
 * The weighting typed in a field, as a whole percentage from 0 to 100, or
 * `null` if it is not one.
 */
export function parseWeight(text: string): number | null {
  const value = text.trim().replace(/\s*%$/, '');
  if (!/^\d{1,3}$/.test(value)) return null;
  const weight = Number(value);
  return weight <= 100 ? weight : null;
}

/** The names of a node's own parents, for context: "Digital, Compliance". */
export function parentNames(node: { parents: readonly { name: string }[] }): string {
  return node.parents.map((p) => p.name).join(', ');
}
