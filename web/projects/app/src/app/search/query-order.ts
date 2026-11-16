/**
 * The `ORDER BY` of a query of the language, read and rewritten as text:
 * sorting a column of the results sorts the query (L06-US01). Strings are
 * skipped, so `summary ~ "order by"` is no sort.
 */

/** How the results are sorted: the first sort key of the query. */
export interface QuerySort {
  readonly field: string;
  readonly descending: boolean;
}

/** Where the top-level `ORDER BY` starts in `query`, outside strings; -1 when none. */
export function orderByIndex(query: string): number {
  let quote: string | null = null;
  for (let i = 0; i < query.length; i++) {
    const c = query[i];
    if (quote) {
      if (c === '\\') i++;
      else if (c === quote) quote = null;
      continue;
    }
    if (c === '"' || c === "'") {
      quote = c;
      continue;
    }
    const before = i === 0 ? ' ' : query[i - 1];
    if (/[\s)]/.test(before) && /^order\s+by\b/i.test(query.slice(i))) return i;
  }
  return -1;
}

/** The first sort key of `query`; `null` when it has no `ORDER BY`. */
export function sortOf(query: string): QuerySort | null {
  const at = orderByIndex(query);
  if (at < 0) return null;
  const keys = query.slice(at).replace(/^order\s+by\s*/i, '');
  const match = /^\s*(?:"((?:[^"\\]|\\.)*)"|([^\s,]+))(?:\s+(asc|desc))?/i.exec(keys);
  if (!match) return null;
  return { field: match[1] ?? match[2], descending: match[3]?.toLowerCase() === 'desc' };
}

/** `query` without its `ORDER BY`. */
export function withoutSort(query: string): string {
  const at = orderByIndex(query);
  return (at < 0 ? query : query.slice(0, at)).trim();
}

/** `query` sorted by `field` only (`asc` or `desc`), or not sorted at all. */
export function withSort(query: string, field: string, direction: 'asc' | 'desc' | ''): string {
  const filter = withoutSort(query);
  if (!direction) return filter;
  const order = `ORDER BY ${field} ${direction.toUpperCase()}`;
  return filter ? `${filter} ${order}` : order;
}
