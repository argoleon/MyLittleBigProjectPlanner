import { Component, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';
import { EmptyState } from 'ui';

/**
 * A section of a configuration not built yet (types, fields, workflows,
 * screens, notifications): what it will hold. Each comes with its story of
 * L04.
 */
@Component({
  selector: 'app-config-section-tab',
  imports: [EmptyState, TranslocoPipe],
  template: `
    <ui-empty-state
      [icon]="icon()"
      [title]="'config.sections.' + section() + '.title' | transloco"
      [message]="'config.sections.' + section() + '.hint' | transloco"
    />
  `,
})
export class ConfigSectionTab {
  /** From the route's data. */
  readonly section = input.required<string>();
  readonly icon = input.required<string>();
}
