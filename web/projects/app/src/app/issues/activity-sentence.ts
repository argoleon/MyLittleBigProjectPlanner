import { ActivityEntry, ActivityValue, formatInstant, formatLocalDate, formatNumber } from 'ui';

/** What formatting a sentence needs: the translations, the locale, the time zone. */
export interface SentenceContext {
  translate: (key: string, params?: Record<string, unknown>) => string;
  locale: string;
  timeZone: string;
}

/** An entry of the history, as the Activity tab says it. */
export interface Sentence {
  /** Who: the member's name, "The system" or "A former member". */
  who: string;
  /** What they did, after their name. */
  what: string;
  /** A rich text before and after (description, rich text field), shown on demand. */
  change: { before: string | null; after: string | null } | null;
  /** The content of a deleted comment, for moderators. */
  deleted: string | null;
}

/** How a value reads in a sentence. */
function valueText(value: ActivityValue | null, context: SentenceContext): string {
  const { translate: t, locale, timeZone } = context;
  if (!value) return t('issue.activity.values.none');
  const text = value.text;
  switch (value.kind) {
    case 'person':
      return text ?? t('issue.activity.values.formerMember');
    case 'issue':
      return text ?? t('issue.activity.values.deletedIssue');
    case 'priority':
      return text ? t(`priority.${text}`) : t('issue.activity.values.none');
    case 'boolean':
      return t(text === 'true' ? 'issue.activity.values.yes' : 'issue.activity.values.no');
    case 'date':
      return text ? formatLocalDate(text, locale) : t('issue.activity.values.none');
    case 'dateTime':
      return text ? formatInstant(text, locale, timeZone, { dateStyle: 'medium', timeStyle: 'short' }) : t('issue.activity.values.none');
    case 'number': {
      const number = Number(text);
      return text !== null && Number.isFinite(number) ? formatNumber(number, locale) : (text ?? t('issue.activity.values.none'));
    }
    default:
      return text ?? t('issue.activity.values.none');
  }
}

/** The field an `updated` entry changed, as it reads in a sentence. */
function fieldName(entry: ActivityEntry, t: SentenceContext['translate']): string {
  if (entry.field === 'custom') return entry.fieldLabel ?? t('issue.activity.fields.removed');
  return t(`issue.activity.fields.${entry.field ?? 'removed'}`);
}

/** Says `entry` in the current language (L05-US06): "changed the status: To do → In progress"… */
export function sentenceOf(entry: ActivityEntry, context: SentenceContext): Sentence {
  const t = context.translate;
  const who = entry.actor?.name ?? t(entry.origin === 'system' ? 'issue.activity.system' : 'issue.activity.formerMember');
  const sentence = (what: string): Sentence => ({ who, what, change: null, deleted: null });
  switch (entry.action) {
    case 'created':
      return sentence(t('issue.activity.created'));
    case 'moved':
      return sentence(t('issue.activity.moved', { old: valueText(entry.old, context), new: valueText(entry.new, context) }));
    case 'link_added':
    case 'link_removed': {
      const other = entry.action === 'link_added' ? entry.new : entry.old;
      const relation = entry.relation ? t(`issue.links.relations.${entry.relation}`) : '';
      return sentence(t(`issue.activity.${entry.action === 'link_added' ? 'linkAdded' : 'linkRemoved'}`, { relation, key: valueText(other, context) }));
    }
    case 'comment_created':
      return sentence(t('issue.activity.commented'));
    case 'comment_updated':
      return sentence(t('issue.activity.commentEdited'));
    case 'comment_deleted':
      return { ...sentence(t('issue.activity.commentDeleted')), deleted: entry.old?.html ?? null };
    case 'updated': {
      const field = fieldName(entry, t);
      if (entry.old?.kind === 'richText' || entry.new?.kind === 'richText') {
        return {
          ...sentence(t('issue.activity.changedText', { field })),
          change: { before: entry.old?.html ?? null, after: entry.new?.html ?? null },
        };
      }
      if (!entry.old) return sentence(t('issue.activity.set', { field, new: valueText(entry.new, context) }));
      if (!entry.new) return sentence(t('issue.activity.cleared', { field, old: valueText(entry.old, context) }));
      return sentence(t('issue.activity.changed', { field, old: valueText(entry.old, context), new: valueText(entry.new, context) }));
    }
  }
}
