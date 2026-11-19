import { BulkSelection } from 'ui';

/** What a bulk action dialog acts on. */
export interface BulkDialogData {
  readonly selection: BulkSelection;
  /** How many tickets are selected. */
  readonly count: number;
}

/** The translation key of a reason a ticket was refused (codes hold dots, keys cannot). */
export function reasonKey(reason: string | null | undefined): string {
  return `bulk.reasons.${(reason ?? 'unknown').replace(/\./g, '_')}`;
}
