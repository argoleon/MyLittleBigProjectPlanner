import { Component, computed, input, output } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';

import { errorTranslationKey, isApiProblem } from '../api/api-problem';

/**
 * What a screen shows when loading failed: the translated message of the
 * error (an `ApiProblem` from the API client, or anything else), and a retry
 * button. Announced to screen readers when it appears.
 */
@Component({
  selector: 'ui-error-state',
  imports: [MatButtonModule, MatIconModule, TranslocoPipe],
  template: `
    <section class="state" role="alert">
      <mat-icon class="icon" aria-hidden="true">error</mat-icon>
      <div class="title" role="heading" [attr.aria-level]="headingLevel()">{{ 'state.errorTitle' | transloco }}</div>
      <p class="message">{{ messageKey() | transloco }}</p>
      @if (retryable()) {
        <div class="actions">
          <button matButton="tonal" type="button" (click)="retry.emit()">{{ 'state.retry' | transloco }}</button>
        </div>
      }
    </section>
  `,
  styleUrls: ['./state.scss', './error-state.scss'],
})
export class ErrorState {
  /** The error caught: an `ApiProblem`, or anything thrown. */
  readonly error = input<unknown>(null);
  /** Whether to offer a retry button. */
  readonly retryable = input(true);
  /** Level of the title in the page outline. */
  readonly headingLevel = input(2);
  readonly retry = output<void>();

  protected readonly messageKey = computed(() => {
    const error = this.error();
    return isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
  });
}
