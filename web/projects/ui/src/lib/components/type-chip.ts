import { Component, computed, input } from '@angular/core';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';

/** Kinds of work item with a dedicated color (`--app-type-*` tokens): a configured type's color. */
export type WorkItemKind = 'story' | 'bug' | 'task' | 'epic';

const ICONS: Readonly<Record<WorkItemKind, string>> = {
  story: 'bookmark',
  bug: 'bug_report',
  task: 'task_alt',
  epic: 'bolt',
};

/** Label of a ticket type, with its icon and color. */
@Component({
  selector: 'ui-type-chip',
  imports: [MatIconModule, TranslocoPipe],
  template: `
    <span class="chip" [attr.data-kind]="kind()">
      <mat-icon class="icon" aria-hidden="true">{{ icon() }}</mat-icon>
      <span class="cdk-visually-hidden">{{ 'type.label' | transloco }} </span>{{ label() ?? ('type.' + kind() | transloco) }}
    </span>
  `,
  styleUrls: ['./chip.scss', './type-chip.scss'],
})
export class TypeChip {
  readonly kind = input.required<WorkItemKind>();
  /** Name of the type as configured in the project; defaults to the kind name. */
  readonly label = input<string | null>(null);
  /** Icon of the type as configured (Material Symbols); defaults to the kind's. */
  readonly iconName = input<string | null>(null);

  protected readonly icon = computed(() => this.iconName() ?? ICONS[this.kind()]);
}
