import { Backlog, BacklogItem, BacklogTotals } from 'ui';

/** A list of the backlog view: an open sprint (by id), or the backlog itself. */
export type ListId = string | 'backlog';

/** The backlog's own list. */
export const BACKLOG: ListId = 'backlog';

/** Where a ticket lands in a list: the one it follows, else the one it precedes. */
export interface Placement {
  readonly afterId: string | null;
  readonly beforeId: string | null;
}

/** The items of `list`. */
export function itemsOf(backlog: Backlog, list: ListId): readonly BacklogItem[] {
  if (list === BACKLOG) return backlog.backlog.items;
  return backlog.sprints.find((s) => s.sprint.id === list)?.items ?? [];
}

/**
 * Where a ticket goes when it takes position `index` in `ids`, the list it
 * lands in without it: right after the ticket above, or before the first
 * one when it goes on top; neither in an empty list (last of the board).
 */
export function placement(ids: readonly string[], index: number): Placement {
  const at = Math.max(0, Math.min(index, ids.length));
  if (at > 0) return { afterId: ids[at - 1] ?? null, beforeId: null };
  return { afterId: null, beforeId: ids[0] ?? null };
}

/** The count and estimate of `items`, as the API sums them. */
export function totalsOf(items: readonly BacklogItem[]): BacklogTotals {
  const totals = { count: 0, estimate: 0, done: 0, unestimated: 0 };
  for (const { card } of items) {
    totals.count += 1;
    if (card.estimate === null || card.estimate === undefined) {
      totals.unestimated += 1;
    } else {
      totals.estimate += card.estimate;
      if (card.status.category === 'done') totals.done += card.estimate;
    }
  }
  return totals;
}

/**
 * The backlog with the ticket `id` moved from list `from` to position `index`
 * of list `to` (counted without it), totals updated: what the screen shows
 * at once, until the backlog is read again. Unchanged when it is not there.
 */
export function moved(
  backlog: Backlog,
  id: string,
  from: ListId,
  to: ListId,
  index: number,
): Backlog {
  const item = itemsOf(backlog, from).find((i) => i.card.id === id);
  if (!item) return backlog;
  const without = (list: ListId) => itemsOf(backlog, list).filter((i) => i.card.id !== id);
  const lists = new Map<ListId, BacklogItem[]>([[from, without(from)]]);
  const target = lists.get(to) ?? without(to);
  target.splice(Math.max(0, Math.min(index, target.length)), 0, item);
  lists.set(to, target);
  const update = (list: ListId, items: BacklogItem[], totals: BacklogTotals) => {
    const changed = lists.get(list);
    return changed ? { items: changed, totals: totalsOf(changed) } : { items, totals };
  };
  return {
    ...backlog,
    backlog: update(BACKLOG, backlog.backlog.items, backlog.backlog.totals),
    sprints: backlog.sprints.map((s) => ({ ...s, ...update(s.sprint.id, s.items, s.totals) })),
  };
}

/** The list a ticket is in. */
export function listOfItem(backlog: Backlog, id: string): ListId | null {
  if (backlog.backlog.items.some((i) => i.card.id === id)) return BACKLOG;
  return backlog.sprints.find((s) => s.items.some((i) => i.card.id === id))?.sprint.id ?? null;
}

/** The lists a planner may move tickets to and from: the backlog and this board's sprints. */
export function plannable(backlog: Backlog, list: ListId): boolean {
  return (
    list === BACKLOG ||
    backlog.sprints.some((s) => s.sprint.id === list && s.own && s.sprint.state !== 'closed')
  );
}
