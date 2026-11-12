import { HolderKind, NodeAccess } from 'ui';

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
 * portfolio Digital").
 */
export function originLabel(access: Pick<NodeAccess, 'direct' | 'origin'>): Label {
  return access.direct
    ? { key: 'hierarchy.access.direct.' + access.origin.type }
    : { key: 'hierarchy.access.inherited.' + access.origin.type, params: { name: access.origin.name } };
}
