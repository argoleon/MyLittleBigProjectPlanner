import { ParsedQuery, QueryNode } from 'ui';

import { CURRENT_USER, Criteria, NO_CRITERIA, criteriaOf, orderByOf, queryOf, written } from './criteria';

/** The tree the API returns for `clauses` joined by AND (as `POST /search/parse` does). */
function parsed(clauses: QueryNode[], orderBy: ParsedQuery['orderBy'] = []): ParsedQuery {
  const filter = clauses.length === 0 ? null : clauses.length === 1 ? clauses[0] : { kind: 'and', operands: clauses };
  return { query: '', filter, orderBy };
}

function clause(field: string, operator: string, ...values: [string, string][]): QueryNode {
  return { kind: 'clause', field, operator, values: values.map(([kind, value]) => ({ kind, value })) };
}

const text = (value: string): [string, string] => ['text', value];
const fn = (value: string): [string, string] => ['function', value];

describe('the criteria of the builder', () => {
  it('write values bare when they read back the same, else quoted', () => {
    expect(['in_progress', 'PLAT', 'PORTF-HR', 'lucas@acme.example', 'Été'].map(written)).toEqual([
      'in_progress',
      'PLAT',
      'PORTF-HR',
      'lucas@acme.example',
      'Été',
    ]);
    expect(['two words', 'and', '2026', '-x', 'say "hi"', 'a\\b', ''].map(written)).toEqual([
      '"two words"',
      '"and"',
      '"2026"',
      '"-x"',
      '"say \\"hi\\""',
      '"a\\\\b"',
      '""',
    ]);
  });

  it('write one clause per criterion, joined by AND, the sort kept', () => {
    const criteria: Criteria = {
      ...NO_CRITERIA,
      projects: ['PLAT'],
      statuses: ['in_progress', 'in_review'],
      assignees: [CURRENT_USER, 'camille@acme.example'],
      priorities: ['high'],
      due: 'this_week',
      created: 'last_30_days',
      text: 'login "SSO"',
    };
    expect(queryOf(criteria, 'ORDER BY priority DESC')).toBe(
      'project IN (PLAT) AND status IN (in_progress, in_review) AND assignee IN (currentUser(), camille@acme.example) ' +
        'AND priority IN (high) AND due >= startOfWeek() AND due <= endOfWeek() AND created >= -30d ' +
        'AND text ~ "login \\"SSO\\"" ORDER BY priority DESC',
    );
    expect(queryOf(NO_CRITERIA)).toBe('');
    expect(queryOf(NO_CRITERIA, 'ORDER BY key ASC')).toBe('ORDER BY key ASC');
  });

  it('write each due and creation preset, and date ranges with their days included', () => {
    const due = (d: Criteria['due']) => queryOf({ ...NO_CRITERIA, due: d });
    expect(due('overdue')).toBe('due < startOfDay()');
    expect(due('this_month')).toBe('due >= startOfMonth() AND due <= endOfMonth()');
    expect(due('none')).toBe('due IS EMPTY');
    expect(due({ from: '2026-11-01', to: '2026-11-30' })).toBe('due >= "2026-11-01" AND due <= "2026-11-30"');
    expect(due({ from: null, to: '2026-11-30' })).toBe('due <= "2026-11-30"');
    const created = (c: Criteria['created']) => queryOf({ ...NO_CRITERIA, created: c });
    expect(created('last_7_days')).toBe('created >= -7d');
    expect(created({ from: '2026-11-01', to: '2026-12-31' })).toBe('created >= "2026-11-01" AND created < "2027-01-01"');
  });

  it('read back what they write', () => {
    expect(
      criteriaOf(
        parsed(
          [
            clause('project', '=', text('PLAT')),
            clause('status', 'IN', text('in_progress')),
            clause('assignee', 'IN', fn('currentUser()'), text('camille@acme.example')),
            clause('due', '>=', text('2026-11-01')),
            clause('due', '<=', text('2026-11-30')),
            clause('created', '<', text('2027-01-01')),
            clause('text', '~', text('outage')),
          ],
          [{ field: 'priority', descending: true }],
        ),
      ),
    ).toEqual({
      ...NO_CRITERIA,
      projects: ['PLAT'],
      statuses: ['in_progress'],
      assignees: [CURRENT_USER, 'camille@acme.example'],
      due: { from: '2026-11-01', to: '2026-11-30' },
      created: { from: null, to: '2026-12-31' },
      text: 'outage',
    });
    expect(criteriaOf(parsed([clause('due', '<', fn('startOfDay()'))]))?.due).toBe('overdue');
    expect(criteriaOf(parsed([clause('due', 'IS EMPTY')]))?.due).toBe('none');
    expect(criteriaOf(parsed([clause('due', '>=', fn('startOfMonth()')), clause('due', '<=', fn('endOfMonth()'))]))?.due).toBe(
      'this_month',
    );
    expect(criteriaOf(parsed([clause('created', '>=', ['relative', '-7d'])]))?.created).toBe('last_7_days');
    expect(criteriaOf(parsed([]))).toEqual(NO_CRITERIA);
    expect(orderByOf(parsed([], [{ field: 'priority', descending: true }, { field: 'key', descending: false }]))).toBe(
      'ORDER BY priority DESC, key ASC',
    );
  });

  it('round-trip through the text the API reads', () => {
    const criteria: Criteria = { ...NO_CRITERIA, types: ['bug', 'story'], due: 'overdue', text: 'x' };
    const query = queryOf(criteria);
    expect(query).toBe('type IN (bug, story) AND due < startOfDay() AND text ~ "x"');
    expect(
      criteriaOf(
        parsed([clause('type', 'IN', text('bug'), text('story')), clause('due', '<', fn('startOfDay()')), clause('text', '~', text('x'))]),
      ),
    ).toEqual(criteria);
  });

  it('leave what they cannot show to the editor', () => {
    const cannot: ParsedQuery[] = [
      { query: '', filter: { kind: 'or', operands: [clause('status', '=', text('todo')), clause('status', '=', text('done'))] }, orderBy: [] },
      { query: '', filter: { kind: 'not', operands: [clause('status', '=', text('todo'))] }, orderBy: [] },
      parsed([clause('status', '!=', text('done'))]),
      parsed([clause('status', '=', text('todo')), clause('status', '=', text('done'))]),
      parsed([clause('summary', '~', text('x'))]),
      parsed([clause('story_points', '>', ['number', '3'])]),
      parsed([clause('project', 'IN', fn('descendantsOf("PORTF-HR")'))]),
      parsed([clause('due', '<', fn('endOfWeek()'))]),
      parsed([clause('due', '>=', fn('startOfWeek()')), clause('due', '<=', fn('endOfMonth()'))]),
      parsed([clause('created', '>=', ['relative', '-3d'])]),
      parsed([clause('due', '>=', text('2026-11-01')), clause('due', '>=', text('2026-11-02'))]),
      parsed([clause('text', '~', text('a')), clause('text', '~', text('b'))]),
    ];
    for (const query of cannot) expect(criteriaOf(query), JSON.stringify(query.filter)).toBeNull();
  });
});
