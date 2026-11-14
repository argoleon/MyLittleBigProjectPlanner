import { ConfigField, FieldKind, FieldMode, FieldRule, IssueType, Workflow, WorkflowStatus } from 'ui';

/** The types of field, in the order they are offered, with their icon (Material Symbols). */
export const FIELD_KINDS: readonly { readonly kind: FieldKind; readonly icon: string }[] = [
  { kind: 'short_text', icon: 'short_text' },
  { kind: 'rich_text', icon: 'notes' },
  { kind: 'number', icon: 'pin' },
  { kind: 'checkbox', icon: 'check_box' },
  { kind: 'date', icon: 'event' },
  { kind: 'date_time', icon: 'schedule' },
  { kind: 'single_select', icon: 'list' },
  { kind: 'multi_select', icon: 'checklist' },
  { kind: 'cascading_select', icon: 'account_tree' },
  { kind: 'user', icon: 'person' },
  { kind: 'group', icon: 'groups' },
  { kind: 'labels', icon: 'sell' },
  { kind: 'url', icon: 'link' },
  { kind: 'issue_link', icon: 'add_link' },
];

/** The icon of a type of field. */
export function kindIcon(kind: FieldKind): string {
  return FIELD_KINDS.find((k) => k.kind === kind)?.icon ?? 'input';
}

/** Whether a field of `kind` picks its values from a list it defines. */
export function hasValues(kind: FieldKind): boolean {
  return kind === 'single_select' || kind === 'multi_select' || kind === 'cascading_select';
}

/** The units of a number field. */
export const UNITS = ['hours', 'days', 'percent'] as const;

/** Most decimals of a number field. */
export const MAX_DECIMALS = 4;

/** Longest short text, and the default limit. */
export const SHORT_TEXT_MAX = 255;

/** Longest label of a field or a list value. */
export const FIELD_LABEL_MAX = 80;

/** A rules grid cell cycles Hidden > Visible > Required > Hidden (mockup 12). */
export function nextMode(mode: FieldMode): FieldMode {
  return mode === 'hidden' ? 'visible' : mode === 'visible' ? 'required' : 'hidden';
}

/** The mode of a cell: its rule, else Visible. `statusId` null: at creation. */
export function modeOf(rules: readonly FieldRule[], issueTypeId: string, statusId: string | null): FieldMode {
  return rules.find((r) => r.issueTypeId === issueTypeId && r.statusId === statusId)?.mode ?? 'visible';
}

/** The rules with the cell set to `mode`: only cells that are not Visible are kept. */
export function withMode(
  rules: readonly FieldRule[],
  issueTypeId: string,
  statusId: string | null,
  mode: FieldMode,
): FieldRule[] {
  const others = rules.filter((r) => !(r.issueTypeId === issueTypeId && r.statusId === statusId));
  return mode === 'visible' ? others : [...others, { issueTypeId, statusId, mode }];
}

/** The rules once the cell is clicked. */
export function cycled(rules: readonly FieldRule[], issueTypeId: string, statusId: string | null): FieldRule[] {
  return withMode(rules, issueTypeId, statusId, nextMode(modeOf(rules, issueTypeId, statusId)));
}

/** Which types a field applies to. */
export interface Applicability {
  readonly allTypes: boolean;
  readonly issueTypeIds: readonly string[];
}

export function appliesTo(applicability: Applicability, typeId: string): boolean {
  return applicability.allTypes || applicability.issueTypeIds.includes(typeId);
}

/** The rules of the types the field still applies to. */
export function rulesFor(rules: readonly FieldRule[], applicability: Applicability): FieldRule[] {
  return rules.filter((r) => appliesTo(applicability, r.issueTypeId));
}

/** One grid of rules: a workflow, its statuses, and the types of the field that follow it. */
export interface RulesGrid {
  readonly workflow: Workflow;
  readonly statuses: readonly WorkflowStatus[];
  readonly types: readonly IssueType[];
}

/** One grid per workflow followed by a type the field applies to, in the order of the workflows and types. */
export function rulesGrids(
  types: readonly IssueType[],
  workflows: readonly Workflow[],
  applicability: Applicability,
): RulesGrid[] {
  return workflows
    .map((workflow) => ({
      workflow,
      statuses: workflow.statuses,
      types: types.filter((t) => t.workflowId === workflow.id && appliesTo(applicability, t.id)),
    }))
    .filter((grid) => grid.types.length > 0);
}

/** The types of a field, in the order of the types; null: all of them. */
export function typesOf(field: Applicability, types: readonly IssueType[]): IssueType[] | null {
  return field.allTypes ? null : types.filter((t) => field.issueTypeIds.includes(t.id));
}

/** The languages a field is labelled in, as their codes in capitals (FR, EN). */
export function languagesOf(field: Pick<ConfigField, 'labelTranslations'>): string[] {
  return ['fr', 'en'].filter((l) => field.labelTranslations[l]?.trim()).map((l) => l.toUpperCase());
}
