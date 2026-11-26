import { NamedRef, NotificationEvent, NotificationRecipientKind, NotificationRule, TransitionRuleReferences, Translations } from 'ui';

/** The events, in their order. */
export const EVENTS: readonly NotificationEvent[] = ['created', 'assigned', 'status_changed', 'commented', 'mentioned', 'absence_conflict'];

/** The people of the ticket a rule may notify. */
export const RECIPIENT_KINDS: readonly NotificationRecipientKind[] = ['reporter', 'assignee', 'watchers', 'mentioned'];

/** The kinds an event offers: the people mentioned only where mentions happen. */
export function kindsFor(event: NotificationEvent): NotificationRecipientKind[] {
  return RECIPIENT_KINDS.filter((kind) => kind !== 'mentioned' || event === 'commented' || event === 'mentioned');
}

/**
 * One recipient of a rule, for its chips: people of the ticket by a
 * translation key; a role, group or member by name (null: deleted since)
 * with its icon.
 */
export interface Recipient {
  readonly key: string | null;
  readonly name: string | null;
  readonly icon: string | null;
}

/** The recipients of a rule, in the order the table shows them. */
export function recipientsOf(
  rule: NotificationRule,
  references: TransitionRuleReferences | null,
  roleLabel: (labels: Translations) => string,
): Recipient[] {
  const named = (list: readonly NamedRef[] | undefined, id: string) => list?.find((n) => n.id === id)?.name ?? null;
  return [
    ...(rule.recipientKinds ?? []).map((kind) => ({ key: `config.notifications.kinds.${kind}`, name: null, icon: null })),
    ...(rule.roleIds ?? []).map((id) => {
      const role = references?.roles.find((r) => r.id === id);
      return { key: null, name: role ? roleLabel(role.labelTranslations) : null, icon: 'badge' };
    }),
    ...(rule.groupIds ?? []).map((id) => ({ key: null, name: named(references?.groups, id), icon: 'groups' })),
    ...(rule.accountIds ?? []).map((id) => ({ key: null, name: named(references?.members, id), icon: 'person' })),
  ];
}

/** Adds `ref` to `list` once. */
export function withRef(list: readonly NamedRef[], ref: NamedRef): NamedRef[] {
  return list.some((r) => r.id === ref.id) ? [...list] : [...list, ref];
}

export function withoutRef(list: readonly NamedRef[], id: string): NamedRef[] {
  return list.filter((r) => r.id !== id);
}
