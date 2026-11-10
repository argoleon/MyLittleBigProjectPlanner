import en from '../../../i18n/en.json';
import fr from '../../../i18n/fr.json';
import { ERROR_CODE } from '../api/generated/models/error-code-array';
import { errorTranslationKey } from '../api/api-problem';

interface Tree {
  [key: string]: string | Tree;
}

function keys(tree: Tree, prefix = ''): string[] {
  return Object.entries(tree).flatMap(([key, value]) =>
    typeof value === 'string' ? [`${prefix}${key}`] : keys(value, `${prefix}${key}.`),
  );
}

function lookup(tree: Tree, key: string): unknown {
  return key.split('.').reduce<unknown>((node, part) => (node as Tree | undefined)?.[part], tree);
}

describe('translation files', () => {
  it('have the same keys in French and English', () => {
    const french = keys(fr);
    const english = keys(en);
    expect(french.filter((k) => !english.includes(k)), 'missing in en.json').toEqual([]);
    expect(english.filter((k) => !french.includes(k)), 'missing in fr.json').toEqual([]);
  });

  it('have no empty text', () => {
    for (const [name, file] of [['fr', fr], ['en', en]] as const) {
      const empty = keys(file).filter((k) => !String(lookup(file, k)).trim());
      expect(empty, name).toEqual([]);
    }
  });

  it('translate every error code of the API, plus the client codes', () => {
    const codes = [...ERROR_CODE, 'network', 'unknown'];
    for (const code of codes) {
      const key = errorTranslationKey(code);
      expect(typeof lookup(fr, key), `fr ${key}`).toBe('string');
      expect(typeof lookup(en, key), `en ${key}`).toBe('string');
    }
  });
});
