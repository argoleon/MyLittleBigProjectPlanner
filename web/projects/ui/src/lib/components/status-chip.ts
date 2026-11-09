import { Component, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';

/** Every workflow status belongs to one of these categories. */
export type StatusCategory = 'todo' | 'progress' | 'done';

/**
 * Label of a ticket status, colored by its category
 * (`--app-status-<category>-*` tokens). The status name itself is configured
 * per project, hence the `label` input.
 */
@Component({
  selector: 'ui-status-chip',
  imports: [TranslocoPipe],
  template: `
    <span class="chip" [attr.data-category]="category()">
      <span class="cdk-visually-hidden">{{ 'status.label' | transloco }} </span>{{ label() ?? ('status.' + category() | transloco) }}
    </span>
  `,
  styleUrls: ['./chip.scss', './status-chip.scss'],
})
export class StatusChip {
  readonly category = input.required<StatusCategory>();
  /** Name of the status, e.g. "In review"; defaults to the category name. */
  readonly label = input<string | null>(null);
}
