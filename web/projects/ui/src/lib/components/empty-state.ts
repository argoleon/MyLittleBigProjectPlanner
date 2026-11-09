import { Component, input } from '@angular/core';
import { MatIconModule } from '@angular/material/icon';

/**
 * What a screen shows when there is nothing to list yet. Actions (e.g. a
 * "Create" button) go in the content:
 *
 * ```html
 * <ui-empty-state icon="inbox" [title]="'tickets.empty' | transloco">
 *   <button matButton="filled">…</button>
 * </ui-empty-state>
 * ```
 */
@Component({
  selector: 'ui-empty-state',
  imports: [MatIconModule],
  template: `
    <section class="state">
      <mat-icon class="icon" aria-hidden="true">{{ icon() }}</mat-icon>
      <div class="title" role="heading" [attr.aria-level]="headingLevel()">{{ title() }}</div>
      @if (message()) {
        <p class="message">{{ message() }}</p>
      }
      <div class="actions"><ng-content /></div>
    </section>
  `,
  styleUrl: './state.scss',
})
export class EmptyState {
  /** Translated title. */
  readonly title = input.required<string>();
  /** Translated explanation. */
  readonly message = input<string | null>(null);
  /** Material Symbols name. */
  readonly icon = input('inbox');
  /** Level of the title in the page outline. */
  readonly headingLevel = input(2);
}
