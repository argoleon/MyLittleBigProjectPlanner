/**
 * Tests of the backlog's own rules (L09-US01): where a dropped ticket lands
 * between its neighbours, the totals of a list, and the backlog redrawn at
 * once after a move.
 */
import { Backlog, BacklogItem, BoardCard } from 'ui';

import { BACKLOG, itemsOf, listOfItem, moved, placement, plannable, totalsOf } from './backlog-model';

function item(n: number, estimate: number | null, category: 'todo' | 'done' = 'todo'): BacklogItem {
  const card = {
    id: `i${n}`,
    key: `PLAT-${n}`,
    summary: `Ticket ${n}`,
    status: { id: 's', key: category, label: category, category },
    estimate,
  } as BoardCard;
  return { card, subtasks: 0 };
}

const TOTALS = { count: 0, estimate: 0, done: 0, unestimated: 0 };

function backlog(): Backlog {
  const sprint = (id: string, own: boolean, items: BacklogItem[]) => ({
    sprint: { id, boardId: own ? 'b1' : 'b2', boardName: own ? 'Mine' : 'Theirs', name: id, state: 'future' as const, version: 1 },
    own,
    items,
    totals: totalsOf(items),
  });
  const rest = [item(3, 5), item(4, null), item(5, 2)];
  return {
    estimation: 'points',
    canPlan: true,
    truncated: false,
    sprints: [sprint('s1', true, [item(1, 8), item(2, 3, 'done')]), sprint('s2', true, []), sprint('x1', false, [item(9, 1)])],
    backlog: { items: rest, totals: totalsOf(rest) },
  };
}

const keys = (items: readonly BacklogItem[]) => items.map((i) => i.card.key);

describe('the backlog model', () => {
  it('places a ticket after the one above it, or before the first on top', () => {
    expect(placement(['a', 'b', 'c'], 0)).toEqual({ afterId: null, beforeId: 'a' });
    expect(placement(['a', 'b', 'c'], 1)).toEqual({ afterId: 'a', beforeId: null });
    expect(placement(['a', 'b', 'c'], 3)).toEqual({ afterId: 'c', beforeId: null });
    expect(placement(['a', 'b', 'c'], 9)).toEqual({ afterId: 'c', beforeId: null });
    expect(placement([], 0)).toEqual({ afterId: null, beforeId: null });
  });

  it('sums the estimates given, the part done, and counts the others', () => {
    expect(totalsOf([])).toEqual(TOTALS);
    expect(totalsOf([item(1, 8), item(2, 3, 'done'), item(3, null), item(4, 0.5)])).toEqual({
      count: 4,
      estimate: 11.5,
      done: 3,
      unestimated: 1,
    });
  });

  it('moves a ticket between lists at once, with their totals', () => {
    const before = backlog();
    const after = moved(before, 'i4', BACKLOG, 's2', 0);
    expect(keys(itemsOf(after, BACKLOG))).toEqual(['PLAT-3', 'PLAT-5']);
    expect(keys(itemsOf(after, 's2'))).toEqual(['PLAT-4']);
    expect(after.backlog.totals).toEqual({ count: 2, estimate: 7, done: 0, unestimated: 0 });
    expect(after.sprints[1].totals).toEqual({ count: 1, estimate: 0, done: 0, unestimated: 1 });
    // Lists left alone keep what was read.
    expect(after.sprints[0]).toEqual(before.sprints[0]);

    const inside = moved(before, 'i1', 's1', 's1', 1);
    expect(keys(itemsOf(inside, 's1'))).toEqual(['PLAT-2', 'PLAT-1']);
    expect(inside.sprints[0].totals).toEqual(before.sprints[0].totals);
    const last = moved(before, 'i1', 's1', BACKLOG, 99);
    expect(keys(itemsOf(last, BACKLOG))).toEqual(['PLAT-3', 'PLAT-4', 'PLAT-5', 'PLAT-1']);
    expect(moved(before, 'nope', BACKLOG, 's1', 0)).toBe(before);
  });

  it('finds a ticket’s list, and which lists a planner may change', () => {
    const b = backlog();
    expect(listOfItem(b, 'i5')).toBe(BACKLOG);
    expect(listOfItem(b, 'i2')).toBe('s1');
    expect(listOfItem(b, 'i9')).toBe('x1');
    expect(listOfItem(b, 'nope')).toBeNull();
    expect(plannable(b, BACKLOG)).toBe(true);
    expect(plannable(b, 's2')).toBe(true);
    expect(plannable(b, 'x1')).toBe(false);
    expect(itemsOf(b, 'unknown')).toEqual([]);
  });
});
