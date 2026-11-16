import { orderByIndex, sortOf, withSort, withoutSort } from './query-order';

describe('the ORDER BY of a query', () => {
  it('is found outside strings only, in any case', () => {
    expect(orderByIndex('status = todo ORDER BY priority DESC')).toBe(14);
    expect(orderByIndex('order by due')).toBe(0);
    expect(orderByIndex('(a IS EMPTY)order   by due')).toBe(12);
    expect(orderByIndex('summary ~ "x ORDER BY y"')).toBe(-1);
    expect(orderByIndex("summary ~ 'it\\'s ORDER BY' ORDER BY key")).toBe(27);
    expect(orderByIndex('recorder by = 1')).toBe(-1);
    expect(orderByIndex('')).toBe(-1);
  });

  it('gives its first key and direction', () => {
    expect(sortOf('status = todo ORDER BY priority DESC, due ASC')).toEqual({ field: 'priority', descending: true });
    expect(sortOf('ORDER BY due')).toEqual({ field: 'due', descending: false });
    expect(sortOf('ORDER BY "order" desc')).toEqual({ field: 'order', descending: true });
    expect(sortOf('status = todo')).toBeNull();
    expect(sortOf('ORDER BY')).toBeNull();
  });

  it('is replaced, or removed, keeping the filter', () => {
    expect(withSort('status = todo ORDER BY priority DESC, due', 'key', 'asc')).toBe('status = todo ORDER BY key ASC');
    expect(withSort('  status = todo  ', 'due', 'desc')).toBe('status = todo ORDER BY due DESC');
    expect(withSort('', 'due', 'desc')).toBe('ORDER BY due DESC');
    expect(withSort('status = todo ORDER BY due', 'due', '')).toBe('status = todo');
    expect(withoutSort('summary ~ "x ORDER BY y"')).toBe('summary ~ "x ORDER BY y"');
  });
});
