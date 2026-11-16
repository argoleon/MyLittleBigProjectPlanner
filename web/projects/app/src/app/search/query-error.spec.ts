import { queryErrorMessage } from './query-error';

const WORDS: Record<string, string> = {
  'search.expected.value': 'une valeur',
  'search.expected.end': 'la fin de la requête',
  'search.expected.or': ' ou ',
};
const translate = (key: string, params?: Record<string, string>) =>
  key === 'search.expected.quoted' ? `« ${params?.['word']} »` : (WORDS[key] ?? key);

describe('the message of a refused query', () => {
  it('says the position from 1, what was expected and what came', () => {
    const message = queryErrorMessage(
      { reason: 'syntax', position: 14, length: 8, expected: ['AND', 'a value', 'the end of the query'], found: 'priority' },
      translate,
    );
    expect(message).toEqual({
      key: 'search.error.syntax',
      params: {
        position: 15,
        expected: '« AND » ou une valeur ou la fin de la requête',
        found: '« priority »',
        field: '',
        allowed: '',
      },
      start: 14,
      end: 22,
    });
  });

  it('names the end of the query, the field and the operators it takes', () => {
    expect(queryErrorMessage({ reason: 'syntax', position: 9, length: 0, expected: ['a value'], found: null }, translate).params['found']).toBe(
      'la fin de la requête',
    );
    const field = queryErrorMessage(
      { reason: 'operator_not_allowed', position: 0, length: 13, found: null, field: 'text', allowed: ['~', '!~'] },
      translate,
    );
    expect([field.key, field.params['field'], field.params['allowed'], field.end]).toEqual([
      'search.error.operator_not_allowed',
      'text',
      '~, !~',
      13,
    ]);
  });
});
