import { ParsedQuery, QueryNode, QueryValue } from 'ui';

/**
 * The criteria of the visual builder (L06-US02) and the queries they write:
 * a fixed row of criteria joined by AND. A query reads back into criteria
 * only when it is made of what the builder writes; any other stays in the
 * editor.
 */

/** Ready-made due dates. */
export type DuePreset = 'overdue' | 'this_week' | 'this_month' | 'none';
/** Ready-made creation dates. */
export type CreatedPreset = 'last_7_days' | 'last_30_days';

/** Local dates `YYYY-MM-DD`, both included; either may be left out. */
export interface DateRange {
  readonly from: string | null;
  readonly to: string | null;
}

export interface Criteria {
  /** Keys. */
  readonly projects: readonly string[];
  readonly types: readonly string[];
  readonly statuses: readonly string[];
  /** Emails, or `currentUser()`. */
  readonly assignees: readonly string[];
  readonly priorities: readonly string[];
  readonly due: DuePreset | DateRange | null;
  readonly created: CreatedPreset | DateRange | null;
  /** Text the tickets hold (`text ~`). */
  readonly text: string;
}

export const NO_CRITERIA: Criteria = {
  projects: [],
  types: [],
  statuses: [],
  assignees: [],
  priorities: [],
  due: null,
  created: null,
  text: '',
};

export const CURRENT_USER = 'currentUser()';

const KEYWORDS = new Set(['and', 'or', 'not', 'in', 'is', 'empty', 'null', 'order', 'by', 'asc', 'desc']);

/** `text` in quotes, a backslash before each `"` and backslash. */
function quoted(text: string): string {
  return `"${text.replace(/[\\"]/g, (c) => `\\${c}`)}"`;
}

/** `text` as the query writes a value: bare when it reads back the same, else quoted. */
export function written(text: string): string {
  const bare =
    /^[\p{L}\p{N}_.@:/][\p{L}\p{N}_.@:/-]*$/u.test(text) &&
    !/^[\d-]/.test(text) &&
    !KEYWORDS.has(text.toLowerCase());
  return bare ? text : quoted(text);
}

function list(field: string, values: readonly string[]): string | null {
  if (values.length === 0) return null;
  const written_ = values.map((v) => (v === CURRENT_USER ? v : written(v)));
  return `${field} IN (${written_.join(', ')})`;
}

/** The day after `date` (`YYYY-MM-DD`). */
function nextDay(date: string): string {
  const [y, m, d] = date.split('-').map(Number);
  const next = new Date(Date.UTC(y, m - 1, d + 1));
  return next.toISOString().slice(0, 10);
}

function previousDay(date: string): string {
  const [y, m, d] = date.split('-').map(Number);
  return new Date(Date.UTC(y, m - 1, d - 1)).toISOString().slice(0, 10);
}

function dueClauses(due: Criteria['due']): string[] {
  switch (due) {
    case null:
      return [];
    case 'overdue':
      return ['due < startOfDay()'];
    case 'this_week':
      return ['due >= startOfWeek()', 'due <= endOfWeek()'];
    case 'this_month':
      return ['due >= startOfMonth()', 'due <= endOfMonth()'];
    case 'none':
      return ['due IS EMPTY'];
    default:
      return [due.from && `due >= "${due.from}"`, due.to && `due <= "${due.to}"`].filter((c): c is string => !!c);
  }
}

function createdClauses(created: Criteria['created']): string[] {
  switch (created) {
    case null:
      return [];
    case 'last_7_days':
      return ['created >= -7d'];
    case 'last_30_days':
      return ['created >= -30d'];
    default:
      // An instant: the end day is included up to its last moment.
      return [created.from && `created >= "${created.from}"`, created.to && `created < "${nextDay(created.to)}"`].filter(
        (c): c is string => !!c,
      );
  }
}

/** The query of `criteria`, sorted by `orderBy` (an `ORDER BY …` clause, or empty). */
export function queryOf(criteria: Criteria, orderBy = ''): string {
  const clauses = [
    list('project', criteria.projects),
    list('type', criteria.types),
    list('status', criteria.statuses),
    list('assignee', criteria.assignees),
    list('priority', criteria.priorities),
    ...dueClauses(criteria.due),
    ...createdClauses(criteria.created),
    criteria.text.trim() ? `text ~ ${quoted(criteria.text.trim())}` : null,
  ].filter((c): c is string => !!c);
  return [clauses.join(' AND '), orderBy.trim()].filter(Boolean).join(' ');
}

const LIST_FIELDS = { project: 'projects', type: 'types', status: 'statuses', assignee: 'assignees', priority: 'priorities' } as const;

const DATE = /^\d{4}-\d{2}-\d{2}$/;

function single(node: QueryNode): QueryValue | null {
  return node.values?.length === 1 ? node.values[0] : null;
}

/**
 * The criteria a parsed query is made of; `null` when the builder cannot
 * show it (OR, NOT, other fields or operators, a field given twice…).
 */
export function criteriaOf(parsed: ParsedQuery): Criteria | null {
  const filter = parsed.filter;
  const clauses: readonly QueryNode[] = !filter ? [] : filter.kind === 'and' ? (filter.operands ?? []) : [filter];
  const criteria: { -readonly [K in keyof Criteria]: Criteria[K] } = { ...NO_CRITERIA };
  const due: Record<string, string> = {};
  const created: Record<string, string> = {};
  const seen = new Set<string>();
  for (const clause of clauses) {
    if (clause.kind !== 'clause' || !clause.field || !clause.operator) return null;
    const { field, operator } = clause;
    const values = clause.values ?? [];
    if (field in LIST_FIELDS && (operator === 'IN' || operator === '=')) {
      if (seen.has(field)) return null;
      seen.add(field);
      const allowed = (v: QueryValue) => v.kind === 'text' || (field === 'assignee' && v.value === CURRENT_USER);
      if (!values.every(allowed)) return null;
      criteria[LIST_FIELDS[field as keyof typeof LIST_FIELDS]] = values.map((v) => v.value);
    } else if (field === 'due' || field === 'created') {
      const into = field === 'due' ? due : created;
      if (operator in into || (operator !== 'IS EMPTY' && !single(clause))) return null;
      into[operator] = single(clause)?.value ?? '';
    } else if (field === 'text' && operator === '~' && single(clause)?.kind === 'text') {
      if (seen.has('text')) return null;
      seen.add('text');
      criteria.text = single(clause)!.value;
    } else {
      return null;
    }
  }
  if (Object.keys(due).length > 0) {
    const read = dueOf(due);
    if (read === undefined) return null;
    criteria.due = read;
  }
  if (Object.keys(created).length > 0) {
    const read = createdOf(created);
    if (read === undefined) return null;
    criteria.created = read;
  }
  return criteria;
}

/** A due criterion from its clauses by operator; `undefined` when none fits. */
function dueOf(by: Record<string, string>): Criteria['due'] | undefined {
  const ops = Object.keys(by).sort().join(' ');
  if (ops === '<' && by['<'] === 'startOfDay()') return 'overdue';
  if (ops === 'IS EMPTY') return 'none';
  if (ops === '<= >=' && by['>='] === 'startOfWeek()' && by['<='] === 'endOfWeek()') return 'this_week';
  if (ops === '<= >=' && by['>='] === 'startOfMonth()' && by['<='] === 'endOfMonth()') return 'this_month';
  const from = by['>='] ?? null;
  const to = by['<='] ?? null;
  const onlyRange = Object.keys(by).every((op) => op === '>=' || op === '<=');
  if (onlyRange && (!from || DATE.test(from)) && (!to || DATE.test(to))) return { from, to };
  return undefined;
}

function createdOf(by: Record<string, string>): Criteria['created'] | undefined {
  const ops = Object.keys(by).sort().join(' ');
  if (ops === '>=' && by['>='] === '-7d') return 'last_7_days';
  if (ops === '>=' && by['>='] === '-30d') return 'last_30_days';
  const from = by['>='] ?? null;
  const before = by['<'] ?? null;
  const onlyRange = Object.keys(by).every((op) => op === '>=' || op === '<');
  if (onlyRange && (!from || DATE.test(from)) && (!before || DATE.test(before))) {
    return { from, to: before ? previousDay(before) : null };
  }
  return undefined;
}

/** The `ORDER BY …` of a parsed query, as the query writes it; empty when none. */
export function orderByOf(parsed: ParsedQuery): string {
  if (parsed.orderBy.length === 0) return '';
  return `ORDER BY ${parsed.orderBy.map((key) => `${key.field} ${key.descending ? 'DESC' : 'ASC'}`).join(', ')}`;
}
