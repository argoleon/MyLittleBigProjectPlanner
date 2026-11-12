import { Component, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';

/**
 * Label of a node role held by a member or a group: tonal when the role is
 * granted on the node shown, neutral when it is inherited from an ancestor.
 */
@Component({
  selector: 'ui-role-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" [attr.data-origin]="inherited() ? 'inherited' : 'direct'">
      <span class="cdk-visually-hidden">{{ 'hierarchy.access.roleLabel' | transloco }} </span>{{ label() }}
    </span>
  `,
  styleUrls: ['./chip.scss', './role-chip.scss'],
})
export class RoleChip {
  /** Name of the role, already in the user's language. */
  readonly label = input.required<string>();
  /** Whether the role comes from an ancestor of the node shown. */
  readonly inherited = input(false);
}
