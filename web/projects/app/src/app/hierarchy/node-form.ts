import { Node, NodeChanges, NodeHealth, NodeOwner, NodeType, fromLocalDate, toLocalDate } from 'ui';

/** Every kind of node, in the order of the hierarchy. */
export const NODE_TYPES: readonly NodeType[] = ['portfolio', 'program', 'project'];

/** Every health, best first. */
export const HEALTHS: readonly NodeHealth[] = ['on_track', 'at_risk', 'off_track'];

/** A project key: 2 to 10 capital letters and digits, a letter first. */
export const PROJECT_KEY = /^[A-Z][A-Z0-9]{1,9}$/;

/** A portfolio or program key: 2 to 20 capital letters, digits and single dashes, a letter first. */
export const NODE_KEY = /^(?=.{2,20}$)[A-Z][A-Z0-9]*(-[A-Z0-9]+)*$/;

/** The key pattern of a node of type `type`. */
export function keyPattern(type: NodeType): RegExp {
  return type === 'project' ? PROJECT_KEY : NODE_KEY;
}

/** The longest key of a node of type `type`. */
export function keyMaxLength(type: NodeType): number {
  return type === 'project' ? 10 : 20;
}

/**
 * The kinds of node that may go under `parent` (specification §4): any at
 * the root or under a portfolio, programs and projects under a program,
 * projects under a project.
 */
export function childTypes(parent: NodeType | null): NodeType[] {
  switch (parent) {
    case 'program':
      return ['program', 'project'];
    case 'project':
      return ['project'];
    default:
      return [...NODE_TYPES];
  }
}

/** A key as typed, made a key: trimmed, in capitals. */
export function normalizeKey(value: string): string {
  return value.trim().toUpperCase();
}

/** What the General tab and the creation dialog edit. */
export interface NodeFormValue {
  readonly name: string;
  /** A project's never changes; a portfolio's or program's may. */
  readonly key: string;
  readonly description: string;
  readonly objectives: string;
  readonly startDate: Date | null;
  readonly endDate: Date | null;
  readonly health: NodeHealth;
  readonly owners: readonly NodeOwner[];
}

/** The form's value for `node`. */
export function formValueOf(node: Node): NodeFormValue {
  return {
    name: node.name,
    key: node.key,
    description: node.description,
    objectives: node.objectives,
    startDate: fromLocalDate(node.startDate),
    endDate: fromLocalDate(node.endDate),
    health: node.health,
    owners: node.owners,
  };
}

/** The API changes of a form value: trimmed name, key in capitals, local dates. */
export function changesOf(value: NodeFormValue): NodeChanges {
  return {
    name: value.name.trim(),
    key: normalizeKey(value.key),
    description: value.description,
    objectives: value.objectives,
    startDate: value.startDate ? toLocalDate(value.startDate) : null,
    endDate: value.endDate ? toLocalDate(value.endDate) : null,
    health: value.health,
    ownerIds: value.owners.map((owner) => owner.accountId),
  };
}

/** Whether two form values would save the same node. */
export function sameChanges(a: NodeFormValue, b: NodeFormValue): boolean {
  return JSON.stringify(changesOf(a)) === JSON.stringify(changesOf(b));
}

/** Whether the end date is before the start date (both set). */
export function endsBeforeStart(start: Date | null, end: Date | null): boolean {
  return !!start && !!end && toLocalDate(end) < toLocalDate(start);
}
