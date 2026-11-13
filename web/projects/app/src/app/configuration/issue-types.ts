import { moveItemInArray } from '@angular/cdk/drag-drop';
import { IssueType, IssueTypeColor } from 'ui';

/** The icons offered for a type (Material Symbols), the defaults first. */
export const TYPE_ICONS: readonly string[] = [
  'bolt',
  'bookmark',
  'task_alt',
  'bug_report',
  'subdirectory_arrow_right',
  'emergency',
  'flag',
  'star',
  'lightbulb',
  'build',
  'description',
  'support_agent',
  'rocket_launch',
  'science',
  'design_services',
  'checklist',
  'security',
  'help',
];

/** The colors of a type, after the type color tokens of the theme. */
export const TYPE_COLORS: readonly IssueTypeColor[] = ['epic', 'story', 'task', 'bug'];

/** The ids of the types under `id`, at any depth. */
export function descendants(types: readonly IssueType[], id: string): Set<string> {
  const found = new Set<string>();
  const pending = [id];
  while (pending.length > 0) {
    const current = pending.pop()!;
    for (const type of types) {
      if (type.parentIds.includes(current) && !found.has(type.id)) {
        found.add(type.id);
        pending.push(type.id);
      }
    }
  }
  return found;
}

/**
 * The types a type may sit under: not a sub-task, not itself, and none of
 * the types under it (it would be its own ancestor). `editing` is null for
 * a new type.
 */
export function parentCandidates(types: readonly IssueType[], editing: string | null): IssueType[] {
  const below = editing ? descendants(types, editing) : new Set<string>();
  return types.filter((type) => !type.isSubtask && type.id !== editing && !below.has(type.id));
}

/** The ids in their order once the type at `from` moves to `to`. */
export function reordered(types: readonly IssueType[], from: number, to: number): string[] {
  const ids = types.map((type) => type.id);
  moveItemInArray(ids, from, to);
  return ids;
}

/** What the form of a type holds. */
export interface TypeDraft {
  readonly key: string;
  readonly labelTranslations: Record<string, string>;
  readonly icon: string;
  readonly color: IssueTypeColor;
  readonly isSubtask: boolean;
  readonly parentIds: readonly string[];
  readonly workflowId: string | null;
}

/** The body of the API: a sub-task keeps its parents, others may have none. */
export function typeFields(draft: TypeDraft) {
  return {
    labelTranslations: draft.labelTranslations,
    icon: draft.icon,
    color: draft.color,
    isSubtask: draft.isSubtask,
    parentIds: [...draft.parentIds],
    workflowId: draft.workflowId,
  };
}
