import { QueryProblem } from 'ui';

/** Why a query was refused, as the screen says it, and the part of the query concerned. */
export interface QueryErrorMessage {
  /** Translation key: `search.error.<reason>`. */
  readonly key: string;
  readonly params: Readonly<Record<string, string | number>>;
  /** The span of the query concerned: `start` included, `end` excluded. */
  readonly start: number;
  readonly end: number;
}

/** The translation keys of the words the API uses for what it expected. */
const EXPECTED_WORDS: Readonly<Record<string, string>> = {
  'a field': 'search.expected.field',
  'a value': 'search.expected.value',
  'a text': 'search.expected.text',
  'the end of the query': 'search.expected.end',
};

/**
 * The message of `problem`: `translate` turns the API's words for what was
 * expected into the member's language; keywords and symbols stay as written.
 */
export function queryErrorMessage(
  problem: QueryProblem,
  translate: (key: string, params?: Record<string, string>) => string,
): QueryErrorMessage {
  const quoted = (word: string) => translate('search.expected.quoted', { word });
  const expected = (problem.expected ?? [])
    .map((word) => (EXPECTED_WORDS[word] ? translate(EXPECTED_WORDS[word]) : quoted(word)))
    .join(translate('search.expected.or'));
  const found = problem.found === null ? translate('search.expected.end') : quoted(problem.found);
  return {
    key: `search.error.${problem.reason}`,
    params: {
      // People count characters from 1.
      position: problem.position + 1,
      expected,
      found,
      field: problem.field ?? '',
      allowed: (problem.allowed ?? []).join(', '),
    },
    start: problem.position,
    end: problem.position + problem.length,
  };
}
