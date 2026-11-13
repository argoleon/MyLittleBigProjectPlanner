import { Node } from 'ui';

import { PROJECT_KEY, changesOf, childTypes, endsBeforeStart, formValueOf, normalizeKey, sameChanges } from './node-form';

const NODE: Node = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7c01',
  type: 'project',
  name: 'Identity platform',
  key: 'PLAT',
  description: 'Accounts and sign-in',
  objectives: 'Single sign-on for all',
  startDate: '2026-01-05',
  endDate: '2026-12-18',
  health: 'at_risk',
  archived: false,
  confidential: false,
  owners: [{ accountId: 'a1', displayName: 'Camille Martin' }],
  parents: [],
  version: 3,
};

describe('node form', () => {
  it('offers the types the matrix allows under a parent', () => {
    expect(childTypes(null)).toEqual(['portfolio', 'program', 'project']);
    expect(childTypes('portfolio')).toEqual(['portfolio', 'program', 'project']);
    expect(childTypes('program')).toEqual(['program', 'project']);
    expect(childTypes('project')).toEqual(['project']);
  });

  it('makes project keys capitals and checks their shape', () => {
    expect(normalizeKey('  plat ')).toBe('PLAT');
    for (const good of ['PLAT', 'ERP2', 'AB', 'ABCDEFGHIJ']) expect(PROJECT_KEY.test(good)).toBe(true);
    for (const bad of ['P', '2PLAT', 'PL-AT', 'ABCDEFGHIJK', 'plat']) expect(PROJECT_KEY.test(bad)).toBe(false);
  });

  it('round-trips a node through the form as local dates', () => {
    const value = formValueOf(NODE);
    expect(value.startDate?.getDate()).toBe(5);
    expect(changesOf(value)).toEqual({
      name: 'Identity platform',
      description: 'Accounts and sign-in',
      objectives: 'Single sign-on for all',
      startDate: '2026-01-05',
      endDate: '2026-12-18',
      health: 'at_risk',
      ownerIds: ['a1'],
    });
    expect(changesOf({ ...value, name: '  Identity  ', startDate: null }).startDate).toBeNull();
    expect(changesOf({ ...value, name: '  Identity  ' }).name).toBe('Identity');
  });

  it('tells unsaved changes apart, names trimmed', () => {
    const value = formValueOf(NODE);
    expect(sameChanges(value, { ...value, name: ' Identity platform ' })).toBe(true);
    expect(sameChanges(value, { ...value, health: 'on_track' })).toBe(false);
    expect(sameChanges(value, { ...value, owners: [] })).toBe(false);
    expect(sameChanges(value, { ...value, endDate: new Date(2026, 11, 19) })).toBe(false);
  });

  it('spots an end before the start, the same day being fine', () => {
    expect(endsBeforeStart(new Date(2026, 5, 2), new Date(2026, 5, 1))).toBe(true);
    expect(endsBeforeStart(new Date(2026, 5, 2), new Date(2026, 5, 2, 23))).toBe(false);
    expect(endsBeforeStart(null, new Date(2026, 5, 1))).toBe(false);
  });
});
