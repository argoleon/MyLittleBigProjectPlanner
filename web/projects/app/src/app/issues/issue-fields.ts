import { FormControl, ValidatorFn, Validators } from '@angular/forms';
import { IssueFieldValue, IssueFormField, IssuePerson, IssueTypeRef, StatusCategory, WorkItemKind, fromLocalDate, toLocalDate } from 'ui';

/** What the control of a custom field holds, by kind. */
export type FieldRaw = string | number | boolean | Date | readonly string[] | readonly IssuePerson[] | null;

/** The kinds of field whose control holds people or groups (`IssuePerson[]`). */
const PEOPLE_KINDS = new Set(['user', 'group']);
/** The kinds whose control holds a list of strings. */
const LIST_KINDS = new Set(['multi_select', 'cascading_select', 'labels']);

/** The empty value of the control of `field`. */
export function emptyOf(field: IssueFormField): FieldRaw {
  if (field.kind === 'checkbox') return false;
  if (PEOPLE_KINDS.has(field.kind) || LIST_KINDS.has(field.kind)) return [];
  return null;
}

/** Whether `raw` holds nothing. */
export function isEmpty(raw: FieldRaw | undefined): boolean {
  if (raw === null || raw === undefined) return true;
  if (typeof raw === 'string') return raw.trim() === '';
  if (Array.isArray(raw)) return raw.length === 0;
  return false;
}

/** A required field must hold something; a checkbox always does, ticked or not (as the server sees it). */
const requiredValue: ValidatorFn = (control) => (isEmpty(control.value as FieldRaw) ? { required: true } : null);

/** The control of a custom field, with the checks the browser can make. */
export function controlOf(field: IssueFormField): FormControl<FieldRaw> {
  const validators: ValidatorFn[] = [];
  if (field.mode === 'required') validators.push(requiredValue);
  if (field.maxLength != null) validators.push(Validators.maxLength(field.maxLength));
  if (field.kind === 'issue_link') validators.push((control) => (isEmpty(control.value as FieldRaw) || isIssueKey(control.value as string) ? null : { format: true }));
  if (field.kind === 'number') {
    if (field.min != null) validators.push(Validators.min(field.min));
    if (field.max != null) validators.push(Validators.max(field.max));
  }
  return new FormControl<FieldRaw>(emptyOf(field), { validators });
}

/**
 * The value of a custom field as the API takes it, `undefined` when there
 * is nothing to send. Ticket links hold the id resolved from the key.
 */
export function apiValue(field: IssueFormField, raw: FieldRaw | undefined): unknown {
  if (field.kind === 'checkbox') return raw === true;
  if (isEmpty(raw)) return undefined;
  switch (field.kind) {
    case 'number':
      return typeof raw === 'number' ? raw : Number(raw);
    case 'date':
      return raw instanceof Date ? toLocalDate(raw) : raw;
    case 'date_time':
      return new Date(raw as string).toISOString();
    case 'user':
    case 'group': {
      const ids = (raw as readonly IssuePerson[]).map((p) => p.id);
      return field.kind === 'user' && field.multiple ? ids : ids[0];
    }
    case 'multi_select':
    case 'cascading_select':
    case 'labels':
      return [...(raw as readonly string[])];
    default:
      return typeof raw === 'string' ? raw.trim() : raw;
  }
}

/** A date-time as a `datetime-local` input takes it, in the browser's time zone. */
function localDateTime(iso: string): string {
  const date = new Date(iso);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** The control value of `field` for the value a ticket holds (none: empty). */
export function rawOf(field: IssueFormField, held: IssueFieldValue | undefined): FieldRaw {
  if (!held || held.value === null || held.value === undefined) return emptyOf(field);
  const value: unknown = held.value;
  switch (field.kind) {
    case 'date':
      return fromLocalDate(value as string);
    case 'date_time':
      return localDateTime(value as string);
    case 'user':
    case 'group':
      return held.references;
    case 'issue_link':
      // Its key; a ticket out of sight shows empty and stays as it is.
      return held.references[0]?.name ?? null;
    case 'multi_select':
    case 'cascading_select':
    case 'labels':
      return Array.isArray(value) ? (value as string[]) : [];
    default:
      return value as FieldRaw;
  }
}

/**
 * The types offered at creation: under a parent, those it may hold;
 * otherwise those that are no sub-task.
 */
export function typesFor(types: readonly IssueTypeRef[], parentTypeId: string | null): IssueTypeRef[] {
  return parentTypeId === null ? types.filter((t) => !t.isSubtask) : types.filter((t) => t.parentIds.includes(parentTypeId));
}

/** The chip category of a status category of the API. */
export function chipCategory(category: string): StatusCategory {
  return category === 'in_progress' ? 'progress' : category === 'done' ? 'done' : 'todo';
}

/** The chip kind of a type color. */
export function chipKind(color: string): WorkItemKind {
  return (['story', 'bug', 'task', 'epic'] as const).find((kind) => kind === color) ?? 'task';
}

/** A ticket key (`PLAT-142`), as the server writes them. */
export function isIssueKey(value: string): boolean {
  return /^[A-Z][A-Z0-9]{1,9}-[1-9][0-9]*$/.test(value.trim());
}
