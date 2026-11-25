import { Component, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';

/**
 * How a resource is held: a person linked to a member's account, a person
 * without account, a former member, or a generic role.
 */
export type ResourceHolding = 'linked' | 'person' | 'former' | 'generic';

/** The holding of a resource, from its kind and account. */
export function resourceHolding(resource: {
  readonly kind: 'person' | 'generic';
  readonly accountId: string | null;
  readonly formerMember: boolean;
}): ResourceHolding {
  if (resource.kind === 'generic') return 'generic';
  if (resource.accountId) return 'linked';
  return resource.formerMember ? 'former' : 'person';
}

/** Label of a resource's kind and account (`--app-account-*` tokens). */
@Component({
  selector: 'ui-resource-kind-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" [attr.data-holding]="holding()">
      <span class="cdk-visually-hidden">{{ 'resource.holding.label' | transloco }} </span
      >{{ 'resource.holding.' + holding() | transloco }}
    </span>
  `,
  styleUrls: ['./chip.scss', './resource-chips.scss'],
})
export class ResourceKindChip {
  readonly holding = input.required<ResourceHolding>();
}

/** Label of a resource's state: active or not (`--app-member-*` tokens). */
@Component({
  selector: 'ui-resource-state-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" [attr.data-active]="active()">
      <span class="cdk-visually-hidden">{{ 'resource.state.label' | transloco }} </span
      >{{ (active() ? 'resource.state.active' : 'resource.state.inactive') | transloco }}
    </span>
  `,
  styleUrls: ['./chip.scss', './resource-chips.scss'],
})
export class ResourceStateChip {
  readonly active = input.required<boolean>();
}
