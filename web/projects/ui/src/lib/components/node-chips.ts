import { Component, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';

/** Kind of node of the hierarchy. */
export type NodeKind = 'portfolio' | 'program' | 'project';

/** How a node stands. */
export type NodeHealthValue = 'on_track' | 'at_risk' | 'off_track';

/** Label of a node's kind (`--app-node-type-*` tokens). */
@Component({
  selector: 'ui-node-type-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" data-node-type>
      <span class="cdk-visually-hidden">{{ 'hierarchy.type.label' | transloco }} </span>{{ 'hierarchy.type.' + type() | transloco }}
    </span>
  `,
  styleUrls: ['./chip.scss', './node-chips.scss'],
})
export class NodeTypeChip {
  readonly type = input.required<NodeKind>();
}

/** Label of a node's health (`--app-health-*` tokens). */
@Component({
  selector: 'ui-health-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" [attr.data-health]="health()">
      <span class="cdk-visually-hidden">{{ 'hierarchy.health.label' | transloco }} </span
      >{{ 'hierarchy.health.' + health() | transloco }}
    </span>
  `,
  styleUrls: ['./chip.scss', './node-chips.scss'],
})
export class HealthChip {
  readonly health = input.required<NodeHealthValue>();
}
