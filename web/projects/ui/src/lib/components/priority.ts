import { Component, computed, input } from '@angular/core';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';

import { IssuePriority } from '../api/generated/models/issue-priority';

const ICONS: Readonly<Record<IssuePriority, string>> = {
  highest: 'keyboard_double_arrow_up',
  high: 'keyboard_arrow_up',
  medium: 'drag_handle',
  low: 'keyboard_arrow_down',
  lowest: 'keyboard_double_arrow_down',
};

/** The icon of a priority. */
export function priorityIcon(priority: IssuePriority): string {
  return ICONS[priority];
}

/** The priority of a ticket: an icon and its name. */
@Component({
  selector: 'ui-priority',
  imports: [MatIconModule, TranslocoPipe],
  template: `
    <span class="priority" [attr.data-priority]="priority()">
      <mat-icon class="icon" aria-hidden="true">{{ icon() }}</mat-icon>
      {{ 'priority.' + priority() | transloco }}
    </span>
  `,
  styleUrl: './priority.scss',
})
export class Priority {
  readonly priority = input.required<IssuePriority>();
  protected readonly icon = computed(() => priorityIcon(this.priority()));
}
