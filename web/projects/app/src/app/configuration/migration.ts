import { MigrationItem, MigrationMapping, MigrationPlan } from 'ui';

/** The target chosen for each removed status and type, by its id; null: none yet. */
export type Choices = Readonly<Record<string, string | null>>;

/** Every removed item, statuses then types. */
function items(plan: MigrationPlan): MigrationItem[] {
  return [...plan.statuses, ...plan.types];
}

/** The suggested target of each removed item, to start with. */
export function initialChoices(plan: MigrationPlan): Record<string, string | null> {
  return Object.fromEntries(items(plan).map((item) => [item.id, item.suggestedId ?? null]));
}

/** Whether every removed item has one of its targets: until then, nothing starts. */
export function isComplete(plan: MigrationPlan, choices: Choices): boolean {
  return items(plan).every((item) => {
    const target = choices[item.id];
    return target !== null && target !== undefined && item.targets.some((t) => t.id === target);
  });
}

/** The tickets to move, for the confirm button: each once, even in both a removed status and a removed type. */
export function ticketTotal(plan: MigrationPlan): number {
  return plan.ticketTotal;
}

/** The mappings for the API, statuses and types apart. */
export function mappings(plan: MigrationPlan, choices: Choices): { statusMappings: MigrationMapping[]; typeMappings: MigrationMapping[] } {
  const of = (list: MigrationItem[]) =>
    list.filter((item) => choices[item.id]).map((item) => ({ fromId: item.id, toId: choices[item.id]! }));
  return { statusMappings: of(plan.statuses), typeMappings: of(plan.types) };
}
