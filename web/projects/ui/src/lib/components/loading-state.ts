import { Component, input } from '@angular/core';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';

/**
 * Loading pattern: an indeterminate progress bar, announced politely to
 * screen readers. Show it in place of the content while it loads:
 *
 * ```html
 * @if (items.isLoading()) { <ui-loading-state /> }
 * @else if (items.error()) { <ui-error-state [error]="items.error()" (retry)="items.reload()" /> }
 * @else if (!items.value()?.items.length) { <ui-empty-state … /> }
 * @else { … }
 * ```
 */
@Component({
  selector: 'ui-loading-state',
  imports: [MatProgressBarModule, TranslocoPipe],
  styleUrl: './loading-state.scss',
  template: `
    <div class="loading" role="status">
      <mat-progress-bar mode="indeterminate" [attr.aria-label]="label() ?? ('state.loading' | transloco)" />
      <span class="cdk-visually-hidden">{{ label() ?? ('state.loading' | transloco) }}</span>
    </div>
  `,
})
export class LoadingState {
  /** Translated text announced while loading; "Loading…" by default. */
  readonly label = input<string | null>(null);
}
