import { LANGUAGES, Language, PERMISSION, Permission, Role, RoleFields } from 'ui';

/** What the role editor holds: one label per language and the permissions. */
export interface RoleDraft {
  readonly labels: Readonly<Record<Language, string>>;
  readonly permissions: readonly Permission[];
}

/** What the editor shows: an existing role, or a new one (blank or copied). */
export type RoleSource =
  | { readonly kind: 'edit'; readonly role: Role }
  | { readonly kind: 'new'; readonly copyOf?: Role };

/**
 * Draft of `source`. A copy keeps the permissions but not the labels, which
 * must stay unique in the tenant.
 */
export function draftOf(source: RoleSource): RoleDraft {
  if (source.kind === 'edit') {
    const labels = Object.fromEntries(
      LANGUAGES.map((language) => [language, source.role.labelTranslations[language] ?? '']),
    ) as Record<Language, string>;
    return { labels, permissions: inCatalogOrder(source.role.permissions) };
  }
  const labels = Object.fromEntries(LANGUAGES.map((language) => [language, ''])) as Record<Language, string>;
  return { labels, permissions: inCatalogOrder(source.copyOf?.permissions ?? []) };
}

/** `permissions` without duplicates, in the order of the catalog. */
export function inCatalogOrder(permissions: Iterable<Permission>): Permission[] {
  const set = new Set(permissions);
  return PERMISSION.filter((permission) => set.has(permission));
}

/** Whether two drafts would save the same role (labels compared trimmed). */
export function sameDraft(a: RoleDraft, b: RoleDraft): boolean {
  return (
    LANGUAGES.every((language) => a.labels[language].trim() === b.labels[language].trim()) &&
    inCatalogOrder(a.permissions).join() === inCatalogOrder(b.permissions).join()
  );
}

/** Body of the API request: labels trimmed, blank ones left out. */
export function bodyOf(draft: RoleDraft): RoleFields {
  const labelTranslations: Record<string, string> = {};
  for (const language of LANGUAGES) {
    const label = draft.labels[language].trim();
    if (label) labelTranslations[language] = label;
  }
  return { labelTranslations, permissions: inCatalogOrder(draft.permissions) };
}

/** What is missing before `draft` can be saved: no label, no permission. */
export function draftProblems(draft: RoleDraft): { noLabel: boolean; noPermission: boolean } {
  return {
    noLabel: LANGUAGES.every((language) => !draft.labels[language].trim()),
    noPermission: draft.permissions.length === 0,
  };
}

/** Label of `role` in `language`, else in another language. */
export function roleLabel(role: Pick<Role, 'labelTranslations'>, language: Language): string {
  const own = role.labelTranslations[language]?.trim();
  if (own) return own;
  for (const other of LANGUAGES) {
    const label = role.labelTranslations[other]?.trim();
    if (label) return label;
  }
  return '';
}
