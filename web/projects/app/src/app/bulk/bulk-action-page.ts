import { Component, DestroyRef, inject, input, resource } from '@angular/core';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ErrorState, LoadingState, getBulkAction, isApiProblem } from 'ui';

import { BulkReport } from './bulk-report';

/** How often a running bulk action is read again. */
export const BULK_POLL_MS = 3_000;

/**
 * The report of a bulk action (L06-US08), where its notification leads:
 * read again while it runs, gone after 7 days.
 */
@Component({
  selector: 'app-bulk-action-page',
  imports: [BulkReport, ErrorState, LoadingState, TranslocoPipe],
  template: `
    @if (bulk.error(); as error) {
      <h1 class="title">{{ 'bulk.report.page' | transloco }}</h1>
      @if (gone(error)) {
        <p class="message" role="status">{{ 'bulk.report.gone' | transloco }}</p>
      } @else {
        <ui-error-state [error]="error" (retry)="bulk.reload()" />
      }
    } @else if (!bulk.hasValue()) {
      <ui-loading-state />
    } @else {
      <h1 class="title">{{ 'bulk.report.title.' + bulk.value().kind | transloco }}</h1>
      @if (bulk.value().status === 'running') {
        <p class="note" role="status">{{ 'bulk.report.running' | transloco }}</p>
      }
      <app-bulk-report [report]="bulk.value()" />
    }
  `,
  styleUrl: './bulk-action-page.scss',
})
export class BulkActionPage {
  /** Id of the bulk action, from the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);

  protected readonly bulk = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getBulkAction, { id: params }),
  });

  constructor() {
    const timer = setInterval(() => {
      if (this.bulk.hasValue() && this.bulk.value().status === 'running') this.bulk.reload();
    }, BULK_POLL_MS);
    inject(DestroyRef).onDestroy(() => clearInterval(timer));
  }

  protected gone(error: unknown): boolean {
    return isApiProblem(error) && error.status === 404;
  }
}
