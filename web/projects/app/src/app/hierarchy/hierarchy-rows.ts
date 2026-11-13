import { HierarchyNode, HierarchyRow } from 'ui';

import { isVeiled } from './veiled';

/** What a row of the tree is doing about its children. */
export type RowState = 'collapsed' | 'loading' | 'expanded' | 'failed';

/**
 * A row of the hierarchy tree. The tree is kept flat, in display order: the
 * children of an expanded row come right after it, one level deeper.
 */
export interface TreeRow {
  /**
   * The ids from the top of the tree down to the row, joined with `/`: a
   * project under two programs shows once under each, as two rows.
   */
  readonly path: string;
  /** Depth of the row: 0 at the top. */
  readonly level: number;
  readonly item: HierarchyRow;
  readonly state: RowState;
}

/** The rows of one level: at the top, or under `parent`. */
export function levelRows(items: readonly HierarchyRow[], parent?: TreeRow): TreeRow[] {
  return items.map((item) => ({
    path: parent ? `${parent.path}/${item.id}` : item.id,
    level: parent ? parent.level + 1 : 0,
    item,
    state: 'collapsed',
  }));
}

/** Whether `row` lies under the row of `path`, at any depth. */
function isUnder(row: TreeRow, path: string): boolean {
  return row.path.startsWith(`${path}/`);
}

/** `rows` with the row of `path` in `state`. */
export function withState(rows: readonly TreeRow[], path: string, state: RowState): TreeRow[] {
  return rows.map((row) => (row.path === path ? { ...row, state } : row));
}

/**
 * `rows` with the row of `path` expanded: its `children` come right after
 * it. Unchanged if the row is gone (the level was reloaded meanwhile).
 */
export function expanded(rows: readonly TreeRow[], path: string, children: readonly HierarchyRow[]): TreeRow[] {
  const index = rows.findIndex((row) => row.path === path);
  if (index < 0) return [...rows];
  const parent: TreeRow = { ...rows[index], state: 'expanded' };
  // Children it already had (expanded twice) are replaced.
  const others = rows.filter((row) => !isUnder(row, path));
  const at = others.findIndex((row) => row.path === path);
  return [...others.slice(0, at), parent, ...levelRows(children, parent), ...others.slice(at + 1)];
}

/** `rows` with the row of `path` collapsed: everything under it leaves. */
export function collapsed(rows: readonly TreeRow[], path: string): TreeRow[] {
  return withState(
    rows.filter((row) => !isUnder(row, path)),
    path,
    'collapsed',
  );
}

/** Whether `row` has a next level to show: a node the user sees, with children. */
export function canExpand(row: TreeRow): row is TreeRow & { item: HierarchyNode } {
  return !isVeiled(row.item) && row.item.hasChildren;
}

/** A load in minutes, in whole hours. */
export function hoursOf(minutes: number): number {
  return Math.round(minutes / 60);
}

/** How a percentage (0 to 100, or more) is formatted: `86 %` in French, `86%` in English. */
export const PERCENT: Intl.NumberFormatOptions = { style: 'percent', maximumFractionDigits: 0 };
