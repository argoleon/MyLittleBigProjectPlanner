import { Component, DestroyRef, inject, input, resource } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ErrorState, InstantPipe, LoadingState, Realtime, getExport, isApiProblem } from 'ui';

/** How often a queued export is read again, besides the real-time signals. */
export const EXPORT_POLL_MS = 5_000;

/**
 * An export of the member (L06-US06), where its notification and email
 * lead: being written, ready to download until it expires, or failed with
 * the reason; gone once expired.
 */
@Component({
  selector: 'app-export-page',
  imports: [ErrorState, InstantPipe, LoadingState, MatButtonModule, MatIconModule, TranslocoPipe],
  template: `
    <h1 class="title">{{ 'exports.title' | transloco }}</h1>
    @if (export.error(); as error) {
      @if (gone(error)) {
        <p class="message" role="status">{{ 'exports.gone' | transloco }}</p>
      } @else {
        <ui-error-state [error]="error" (retry)="export.reload()" />
      }
    } @else if (!export.hasValue()) {
      <ui-loading-state />
    } @else {
      @let e = export.value();
      <section class="card" [attr.aria-labelledby]="'export-file'">
        <h2 class="file" id="export-file">{{ e.fileName }}</h2>
        @switch (e.status) {
          @case ('ready') {
            <p>{{ 'exports.ready' | transloco: { count: e.rows ?? 0 } }}</p>
            <p class="note">{{ 'exports.expires' | transloco: { at: (e.expiresAt | instant: { dateStyle: 'medium', timeStyle: 'short' }) } }}</p>
            <a matButton="filled" class="download" [href]="e.downloadPath">
              <mat-icon aria-hidden="true">download</mat-icon>
              {{ 'exports.download' | transloco }}
            </a>
          }
          @case ('queued') {
            <p role="status">{{ 'exports.preparing' | transloco }}</p>
          }
          @case ('failed') {
            <p class="message error" role="alert">{{ failureKey(e.failure) | transloco }}</p>
          }
        }
      </section>
    }
  `,
  styleUrl: './export-page.scss',
})
export class ExportPage {
  /** Id of the export, from the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);

  protected readonly export = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getExport, { id: params }),
  });

  constructor() {
    const reloadIfQueued = () => {
      if (this.export.hasValue() && this.export.value().status === 'queued') this.export.reload();
    };
    inject(Realtime)
      .events.pipe(takeUntilDestroyed())
      .subscribe((event) => {
        if (event.type !== 'issue') reloadIfQueued();
      });
    const timer = setInterval(reloadIfQueued, EXPORT_POLL_MS);
    inject(DestroyRef).onDestroy(() => clearInterval(timer));
  }

  /** The message of a failure code. */
  protected failureKey(code: string | null | undefined): string {
    const known: Record<string, string> = { 'query.invalid': 'queryInvalid', 'export.too_large': 'tooLarge' };
    return `exports.failure.${known[code ?? ''] ?? 'unknown'}`;
  }

  /** Expired, or not the member's. */
  protected gone(error: unknown): boolean {
    return isApiProblem(error) && error.status === 404;
  }
}
