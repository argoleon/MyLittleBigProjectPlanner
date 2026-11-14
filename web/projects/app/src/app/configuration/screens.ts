import { moveItemInArray } from '@angular/cdk/drag-drop';
import { ConfigField, ConfigScreen, FieldMode, IssueType, ScreenKind, Workflow } from 'ui';

import { appliesTo, modeOf } from './fields';

/** The kinds of screen, in the order the tab shows them. */
export const SCREEN_KINDS: readonly ScreenKind[] = ['creation', 'view', 'transition'];

/** Longest name of a screen. */
export const SCREEN_NAME_MAX = 60;

/** The mode of `field` for a ticket of `typeId` at a step (`statusId` null: creation). */
export function fieldMode(field: ConfigField, typeId: string, statusId: string | null): FieldMode {
  return appliesTo(field, typeId) ? modeOf(field.rules, typeId, statusId) : 'hidden';
}

/** A line of the preview of a screen. */
export interface PreviewLine {
  readonly field: ConfigField;
  readonly mode: FieldMode;
  /** Required at the step but missing from the screen: added at the end. */
  readonly added: boolean;
}

/**
 * What a screen shows for a ticket of `typeId` at a step, as the API
 * resolves it, with the fields the rules hide kept (mode `hidden`) for the
 * preview: the screen's fields in their order, then the required ones it
 * lacks.
 */
export function preview(
  fieldIds: readonly string[],
  fields: readonly ConfigField[],
  typeId: string,
  statusId: string | null,
): PreviewLine[] {
  const own = fieldIds
    .map((id) => fields.find((f) => f.id === id))
    .filter((f): f is ConfigField => f !== undefined)
    .map((field) => ({ field, mode: fieldMode(field, typeId, statusId), added: false }));
  const added = fields
    .filter((f) => !fieldIds.includes(f.id) && fieldMode(f, typeId, statusId) === 'required')
    .map((field) => ({ field, mode: 'required' as FieldMode, added: true }));
  return [...own, ...added];
}

/** The types a screen is shown for: its own, or every type (a general or transition screen). */
export function typesOf(screen: Pick<ConfigScreen, 'issueTypeId'>, types: readonly IssueType[]): IssueType[] {
  return screen.issueTypeId ? types.filter((t) => t.id === screen.issueTypeId) : [...types];
}

/** The steps (status ids; null: creation) a screen of `kind` is shown at for `type`. */
export function stepsOf(kind: ScreenKind, type: IssueType, workflows: readonly Workflow[]): (string | null)[] {
  if (kind === 'creation') return [null];
  return workflows.find((w) => w.id === type.workflowId)?.statuses.map((s) => s.id) ?? [];
}

/**
 * The fields required somewhere the screen is shown but missing from it:
 * they will be added at the end there. For a transition screen, every
 * status counts (it depends on the transition it is attached to).
 */
export function missingRequired(
  screen: Pick<ConfigScreen, 'kind' | 'issueTypeId'>,
  fieldIds: readonly string[],
  fields: readonly ConfigField[],
  types: readonly IssueType[],
  workflows: readonly Workflow[],
): ConfigField[] {
  return fields.filter(
    (field) =>
      !fieldIds.includes(field.id) &&
      typesOf(screen, types).some((type) =>
        stepsOf(screen.kind, type, workflows).some((step) => fieldMode(field, type.id, step) === 'required'),
      ),
  );
}

/** The fields not on the screen yet, in the order of the configuration. */
export function available(fieldIds: readonly string[], fields: readonly ConfigField[]): ConfigField[] {
  return fields.filter((f) => !fieldIds.includes(f.id));
}

/** The field ids once `id` is added at `index` (at the end by default). */
export function withField(fieldIds: readonly string[], id: string, index = fieldIds.length): string[] {
  if (fieldIds.includes(id)) return [...fieldIds];
  const ids = [...fieldIds];
  ids.splice(Math.max(0, Math.min(index, ids.length)), 0, id);
  return ids;
}

export function withoutField(fieldIds: readonly string[], id: string): string[] {
  return fieldIds.filter((f) => f !== id);
}

/** The field ids once the one at `from` moves to `to`. */
export function moved(fieldIds: readonly string[], from: number, to: number): string[] {
  const ids = [...fieldIds];
  if (to >= 0 && to < ids.length) moveItemInArray(ids, from, to);
  return ids;
}
