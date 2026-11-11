import { Role } from 'ui';

import { RoleDraft, bodyOf, draftOf, draftProblems, inCatalogOrder, roleLabel, sameDraft } from './role-draft';

const MANAGER: Role = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01',
  label: 'Project manager',
  labelTranslations: { fr: 'Chef de projet', en: 'Project manager' },
  permissions: ['view', 'edit', 'comment'],
  grantCount: 0,
  version: 2,
};

describe('role drafts', () => {
  it('loads an existing role with its labels and permissions', () => {
    expect(draftOf({ kind: 'edit', role: MANAGER })).toEqual({
      labels: { fr: 'Chef de projet', en: 'Project manager' },
      permissions: ['view', 'edit', 'comment'],
    });
  });

  it('leaves a missing translation blank', () => {
    const draft = draftOf({ kind: 'edit', role: { ...MANAGER, labelTranslations: { fr: 'Chef de projet' } } });
    expect(draft.labels).toEqual({ fr: 'Chef de projet', en: '' });
  });

  it('starts a new role blank, and a copy with the permissions only', () => {
    expect(draftOf({ kind: 'new' })).toEqual({ labels: { fr: '', en: '' }, permissions: [] });
    expect(draftOf({ kind: 'new', copyOf: MANAGER })).toEqual({
      labels: { fr: '', en: '' },
      permissions: ['view', 'edit', 'comment'],
    });
  });

  it('orders permissions as the catalog and drops duplicates', () => {
    expect(inCatalogOrder(['comment', 'view', 'administer_node', 'view'])).toEqual(['view', 'comment', 'administer_node']);
  });

  describe('dirty state', () => {
    const base = draftOf({ kind: 'edit', role: MANAGER });

    it('is clean when nothing changed', () => {
      expect(sameDraft(base, { ...base })).toBe(true);
    });

    it('is dirty when a label or a permission changes', () => {
      expect(sameDraft(base, { ...base, labels: { ...base.labels, en: 'Manager' } })).toBe(false);
      expect(sameDraft(base, { ...base, permissions: [...base.permissions, 'log_time'] })).toBe(false);
      expect(sameDraft(base, { ...base, permissions: ['view'] })).toBe(false);
    });

    it('ignores spaces around labels and the order of permissions', () => {
      const same: RoleDraft = {
        labels: { fr: ' Chef de projet ', en: 'Project manager  ' },
        permissions: ['comment', 'view', 'edit'],
      };
      expect(sameDraft(base, same)).toBe(true);
    });

    it('is clean again once a toggle is switched back', () => {
      const toggled = { ...base, permissions: [...base.permissions, 'log_time' as const] };
      const back = { ...toggled, permissions: toggled.permissions.filter((p) => p !== 'log_time') };
      expect(sameDraft(base, back)).toBe(true);
    });
  });

  it('saves trimmed labels, without the blank ones, and ordered permissions', () => {
    expect(bodyOf({ labels: { fr: ' Lecteur ', en: '  ' }, permissions: ['comment', 'view'] })).toEqual({
      labelTranslations: { fr: 'Lecteur' },
      permissions: ['view', 'comment'],
    });
  });

  it('needs a label and a permission', () => {
    expect(draftProblems({ labels: { fr: ' ', en: '' }, permissions: [] })).toEqual({
      noLabel: true,
      noPermission: true,
    });
    expect(draftProblems({ labels: { fr: '', en: 'Reader' }, permissions: ['view'] })).toEqual({
      noLabel: false,
      noPermission: false,
    });
  });

  it('shows the label of the language, else another one', () => {
    expect(roleLabel(MANAGER, 'fr')).toBe('Chef de projet');
    expect(roleLabel(MANAGER, 'en')).toBe('Project manager');
    expect(roleLabel({ labelTranslations: { fr: 'Lecteur', en: ' ' } }, 'en')).toBe('Lecteur');
    expect(roleLabel({ labelTranslations: {} }, 'en')).toBe('');
  });
});
