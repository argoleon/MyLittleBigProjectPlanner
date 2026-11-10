import { Component, input } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';
import { EmptyState } from 'ui';

/** Placeholder of a screen that comes with a later work package. */
@Component({
  selector: 'app-coming-soon-page',
  imports: [EmptyState, TranslocoPipe],
  styleUrl: './coming-soon-page.scss',
  template: `
    <h1 class="title">{{ titleKey() | transloco }}</h1>
    <ui-empty-state
      icon="construction"
      [title]="'page.comingSoon.title' | transloco"
      [message]="'page.comingSoon.message' | transloco"
    />
  `,
})
export class ComingSoonPage {
  /** From the route data. */
  readonly titleKey = input.required<string>();
}
