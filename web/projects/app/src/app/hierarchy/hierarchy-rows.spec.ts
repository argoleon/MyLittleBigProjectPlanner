import { HierarchyNode, HierarchyRow, NodeAggregate, VeiledHierarchyNode } from 'ui';

import { TreeRow, canExpand, collapsed, expanded, hoursOf, levelRows, withState } from './hierarchy-rows';

const AGGREGATE: NodeAggregate = {
  projects: 1,
  confidentialProjects: 0,
  plannedMinutes: 600,
  capacityMinutes: 0,
  capacityUsedPercent: null,
  openTickets: 3,
  lateTickets: 0,
  progressPercent: 25,
  confidentialOpenTickets: 0,
};

function node(id: string, hasChildren = false): HierarchyNode {
  return {
    id,
    type: hasChildren ? 'program' : 'project',
    name: id,
    health: 'on_track',
    owners: [],
    otherParents: [],
    hasChildren,
    aggregate: AGGREGATE,
  };
}

const VEILED: VeiledHierarchyNode = { veiled: true, id: 'opaque', type: 'project', plannedMinutes: 600 };

/** The rows as `path:level:state`, in display order. */
function shape(rows: readonly TreeRow[]): string[] {
  return rows.map((row) => `${row.path}:${row.level}:${row.state}`);
}

describe('the rows of the hierarchy tree', () => {
  const top: HierarchyRow[] = [node('a', true), node('b', true), node('lone')];

  it('start folded, at the top', () => {
    expect(shape(levelRows(top))).toEqual(['a:0:collapsed', 'b:0:collapsed', 'lone:0:collapsed']);
    expect(levelRows([])).toEqual([]);
  });

  it('show the children of an expanded row right after it, one level deeper', () => {
    const rows = expanded(levelRows(top), 'a', [node('plat'), VEILED]);
    expect(shape(rows)).toEqual(['a:0:expanded', 'a/plat:1:collapsed', 'a/opaque:1:collapsed', 'b:0:collapsed', 'lone:0:collapsed']);
    expect(rows[2].item).toBe(VEILED);
  });

  it('give a node under two parents one row under each', () => {
    let rows = expanded(levelRows(top), 'a', [node('plat', true)]);
    rows = expanded(rows, 'b', [node('plat', true)]);
    expect(shape(rows)).toEqual(['a:0:expanded', 'a/plat:1:collapsed', 'b:0:expanded', 'b/plat:1:collapsed', 'lone:0:collapsed']);

    // Each of the two rows expands by itself.
    rows = expanded(rows, 'b/plat', [node('sub')]);
    expect(shape(rows)).toEqual([
      'a:0:expanded',
      'a/plat:1:collapsed',
      'b:0:expanded',
      'b/plat:1:expanded',
      'b/plat/sub:2:collapsed',
      'lone:0:collapsed',
    ]);
  });

  it('fold a row with everything under it, and nothing else', () => {
    let rows = expanded(levelRows(top), 'a', [node('plat', true)]);
    rows = expanded(rows, 'a/plat', [node('sub')]);
    rows = expanded(rows, 'b', [node('other')]);
    expect(shape(collapsed(rows, 'a'))).toEqual(['a:0:collapsed', 'b:0:expanded', 'b/other:1:collapsed', 'lone:0:collapsed']);
    expect(shape(collapsed(rows, 'a/plat'))).toEqual([
      'a:0:expanded',
      'a/plat:1:collapsed',
      'b:0:expanded',
      'b/other:1:collapsed',
      'lone:0:collapsed',
    ]);
    // An id that starts like another is not under it.
    const alike = levelRows([node('a', true), node('ab')]);
    expect(shape(collapsed(alike, 'a'))).toEqual(['a:0:collapsed', 'ab:0:collapsed']);
  });

  it('replace the children of a row expanded again', () => {
    let rows = expanded(levelRows(top), 'a', [node('plat')]);
    rows = expanded(rows, 'a', [node('portal'), node('plat')]);
    expect(shape(rows).slice(0, 3)).toEqual(['a:0:expanded', 'a/portal:1:collapsed', 'a/plat:1:collapsed']);
    expect(rows.length).toBe(5);
  });

  it('leave the tree as it is when the row is gone', () => {
    const rows = levelRows(top);
    expect(expanded(rows, 'gone', [node('plat')])).toEqual(rows);
    expect(collapsed(rows, 'gone')).toEqual(rows);
  });

  it('track what a row is doing about its children', () => {
    const rows = withState(levelRows(top), 'b', 'loading');
    expect(shape(rows)).toEqual(['a:0:collapsed', 'b:0:loading', 'lone:0:collapsed']);
    expect(shape(withState(rows, 'b', 'failed'))[1]).toBe('b:0:failed');
  });

  it('only expand nodes the user sees that have children', () => {
    const [program, , lone] = levelRows(top);
    expect(canExpand(program)).toBe(true);
    expect(canExpand(lone)).toBe(false);
    expect(canExpand(levelRows([VEILED])[0])).toBe(false);
  });
});

describe('hoursOf', () => {
  it('rounds a load in minutes to whole hours', () => {
    expect(hoursOf(0)).toBe(0);
    expect(hoursOf(212_400)).toBe(3540);
    expect(hoursOf(89)).toBe(1);
    expect(hoursOf(90)).toBe(2);
    expect(hoursOf(29)).toBe(0);
  });
});
