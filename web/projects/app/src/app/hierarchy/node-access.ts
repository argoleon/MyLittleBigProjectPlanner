import { HolderKind, NodeAccess, NodeType } from 'ui';

/** Material Symbols icon of who holds a role. */
export const HOLDER_ICONS: Record<HolderKind, string> = {
  member: 'person',
  group: 'group',
};

/** A translation key with its parameters. */
export interface Label {
  readonly key: string;
  readonly params?: Record<string, string>;
}

/**
 * Where a grant that applies on a node comes from: made on the node itself
 * ("Direct role on this project"), or on an ancestor ("Inherited from the
 * portfolio Digital"). An ancestor the user does not see is not named: its
 * origin is `null`.
 *
 * `nodeType` is the kind of the node whose access is shown.
 */
export function originLabel(access: Pick<NodeAccess, 'direct' | 'origin'>, nodeType: NodeType): Label {
  if (access.direct) return { key: 'hierarchy.access.direct.' + nodeType };
  const origin = access.origin;
  return origin
    ? { key: 'hierarchy.access.inherited.' + origin.type, params: { name: origin.name } }
    : { key: 'hierarchy.access.inheritedHidden' };
}
