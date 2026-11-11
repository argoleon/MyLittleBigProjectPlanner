import { Component, DOCUMENT, InjectionToken, inject, input, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  LocaleService,
  Me,
  downloadMyDataExport,
  errorTranslationKey,
  exportMyData,
  getMyDataExport,
  isApiProblem,
} from 'ui';

import { ErasureDialog } from './erasure-dialog';

/** Saves `content` as a file named `name` on the user's device. Replaced in tests. */
export const SAVE_FILE = new InjectionToken<(name: string, content: Blob) => void>('SAVE_FILE', {
  providedIn: 'root',
  factory: () => {
    const document = inject(DOCUMENT);
    return (name: string, content: Blob) => {
      const url = URL.createObjectURL(content);
      const link = document.createElement('a');
      link.href = url;
      link.download = name;
      document.body.append(link);
      link.click();
      link.remove();
      URL.revokeObjectURL(url);
    };
  },
});

/** Name of an export file: `personal-data-2026-11-11.json`, the date in `timeZone`. */
export function exportFileName(now: Date, timeZone: string): string {
  // en-CA writes dates as YYYY-MM-DD.
  return `personal-data-${new Intl.DateTimeFormat('en-CA', { timeZone }).format(now)}.json`;
}

/** Milliseconds between two looks at an export being collected. */
export const EXPORT_POLL_MS = new InjectionToken<number>('EXPORT_POLL_MS', {
  providedIn: 'root',
  factory: () => 1000,
});

/** Looks at an export at most this many times (a minute by default). */
const MAX_POLLS = 60;

/**
 * "My personal data" (mockup 10, L02-US09): the export of the account's
 * data as JSON (right of access), and the erasure of the account (right to
 * erasure).
 */
@Component({
  selector: 'app-personal-data-card',
  imports: [MatButtonModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  styleUrl: './personal-data-card.scss',
  template: `
    <section class="card" aria-labelledby="personal-data-title">
      <h2 id="personal-data-title" class="card-title">{{ 'account.personalData.title' | transloco }}</h2>
      <p class="intro">{{ 'account.personalData.intro' | transloco }}</p>
      @if (exporting()) {
        <mat-progress-bar mode="indeterminate" [attr.aria-label]="'account.personalData.exporting' | transloco" />
      }
      @if (notice(); as notice) {
        <p class="message" [class.info]="notice.ok" [class.error]="!notice.ok" [attr.role]="notice.ok ? 'status' : 'alert'">
          {{ notice.key | transloco }}
        </p>
      }
      <div class="actions">
        <button matButton="outlined" type="button" class="export" [disabled]="exporting()" (click)="exportData()">
          <mat-icon aria-hidden="true">download</mat-icon>
          {{ (exporting() ? 'account.personalData.exporting' : 'account.personalData.export') | transloco }}
        </button>
        <button matButton="outlined" type="button" class="erase" [disabled]="!me()" (click)="erase()">
          {{ 'account.personalData.erase' | transloco }}
        </button>
      </div>
    </section>
  `,
})
export class PersonalDataCard {
  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly dialog = inject(MatDialog);
  private readonly save = inject(SAVE_FILE);
  private readonly locale = inject(LocaleService);
  private readonly pollMs = inject(EXPORT_POLL_MS);

  /** The signed-in account; erasure waits for it. */
  readonly me = input<Me | null>(null);
  protected readonly exporting = signal(false);
  protected readonly notice = signal<{ ok: boolean; key: string } | null>(null);

  /** Asks for an export, follows it until ready, then downloads it. */
  protected async exportData(): Promise<void> {
    this.exporting.set(true);
    this.notice.set(null);
    try {
      let current = await this.api.invoke(exportMyData);
      for (let polls = 0; current.status !== 'ready' && polls < MAX_POLLS; polls++) {
        await new Promise((resolve) => setTimeout(resolve, this.pollMs));
        current = await this.api.invoke(getMyDataExport, { id: current.id });
      }
      if (current.status !== 'ready') {
        this.notice.set({ ok: false, key: 'account.personalData.exportSlow' });
        return;
      }
      // Through the API client: the session's cookie, and errors as problems.
      const data: unknown = await this.api.invoke(downloadMyDataExport, { id: current.id });
      const file = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
      this.save(exportFileName(new Date(), this.locale.timeZone()), file);
      this.notice.set({ ok: true, key: 'account.personalData.exported' });
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.exporting.set(false);
    }
  }

  /** The erasure dialog; once erased, the account is signed out. */
  protected erase(): void {
    const me = this.me();
    if (!me) return;
    this.dialog
      .open<ErasureDialog, Me, boolean>(ErasureDialog, { data: me, width: 'min(560px, 100vw)', autoFocus: 'first-heading' })
      .afterClosed()
      .subscribe((erased) => {
        if (erased) void this.auth.accountErased();
      });
  }
}
