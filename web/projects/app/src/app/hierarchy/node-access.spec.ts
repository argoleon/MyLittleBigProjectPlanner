import { HOLDER_ICONS, originLabel } from './node-access';

describe('originLabel', () => {
  it('says a direct role is on this kind of node', () => {
    for (const type of ['portfolio', 'program', 'project'] as const) {
      expect(originLabel({ direct: true, origin: { id: 'n', type, name: 'Here' } }, type)).toEqual({
        key: `hierarchy.access.direct.${type}`,
      });
    }
  });

  it('names the ancestor an inherited role comes from, with its kind', () => {
    expect(originLabel({ direct: false, origin: { id: 'p', type: 'portfolio', name: 'Digital' } }, 'project')).toEqual({
      key: 'hierarchy.access.inherited.portfolio',
      params: { name: 'Digital' },
    });
    expect(originLabel({ direct: false, origin: { id: 'p', type: 'program', name: 'Compliance' } }, 'project')).toEqual({
      key: 'hierarchy.access.inherited.program',
      params: { name: 'Compliance' },
    });
  });

  it('does not name an ancestor the user has no access to', () => {
    expect(originLabel({ direct: false, origin: null }, 'project')).toEqual({
      key: 'hierarchy.access.inheritedHidden',
    });
  });
});

describe('HOLDER_ICONS', () => {
  it('tells members from groups', () => {
    expect(HOLDER_ICONS).toEqual({ member: 'person', group: 'group' });
  });
});
