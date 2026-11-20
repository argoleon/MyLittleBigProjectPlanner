import { Component, inject, input, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { MatProgressSpinnerModule } from '@angular/material/progress-spinner';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ExportFormat, ListExport, createExport, errorTranslationKey, isApiProblem } from 'ui';

import { FileDownload } from './file-download';

/**
 * The Export menu of a list view (L06-US06, L06-US07): its tickets, as the
 * query finds them, with the columns displayed, as CSV or Excel. A small export downloads at once;
 * a large one is written in the background, the member notified when it is
 * ready (and told so here).
 */
@Component({
  selector: 'app-export-menu',
  imports: [MatButtonModule, MatIconModule, MatMenuModule, MatProgressSpinnerModule, RouterLink, TranslocoPipe],
  template: `
    <button matButton="outlined" type="button" class="export" [matMenuTriggerFor]="formats" [disabled]="disabled() || running()">
      @if (running()) {
        <mat-progress-spinner mode="indeterminate" diameter="18" [attr.aria-label]="'exports.running' | transloco" />
      } @else {
        <mat-icon aria-hidden="true">download</mat-icon>
      }
      {{ 'exports.button' | transloco }}
    </button>
    <mat-menu #formats="matMenu">
      <button mat-menu-item type="button" class="csv" (click)="export('csv')">
        <mat-icon aria-hidden="true">table_view</mat-icon>
        {{ 'exports.csv' | transloco }}
      </button>
      <button mat-menu-item type="button" class="xlsx" (click)="export('xlsx')">
        <mat-icon aria-hidden="true">grid_on</mat-icon>
        {{ 'exports.xlsx' | transloco }}
      </button>
    </mat-menu>
    @if (queued(); as queued) {
      <p class="message info" role="status">
        {{ 'exports.queued' | transloco: { file: queued.fileName } }}
        <a [routerLink]="['/exports', queued.id]">{{ 'exports.follow' | transloco }}</a>
      </p>
    }
    @if (errorKey(); as key) {
      <p class="message error" role="alert">{{ key | transloco }}</p>
    }
  `,
  styleUrl: './export-menu.scss',
})
export class ExportMenu {
  /** The query of the list. */
  readonly query = input.required<string>();
  /** The columns displayed, in their order (`key`, `summary`…). */
  readonly columns = input.required<readonly string[]>();
  /** What the file is named after. */
  readonly name = input('export');
  readonly disabled = input(false);

  private readonly api = inject(Api);
  private readonly download = inject(FileDownload);

  protected readonly running = signal(false);
  protected readonly queued = signal<ListExport | null>(null);
  protected readonly errorKey = signal<string | null>(null);

  protected async export(format: ExportFormat): Promise<void> {
    this.running.set(true);
    this.queued.set(null);
    this.errorKey.set(null);
    try {
      const created = await this.api.invoke(createExport, {
        body: { query: this.query(), columns: [...this.columns()], format, name: this.name() },
      });
      if (created.status === 'ready' && created.downloadPath) this.download.start(created.downloadPath);
      else this.queued.set(created);
    } catch (error) {
      if (isApiProblem(error) && error.code === 'export.too_large') {
        this.errorKey.set('exports.tooLarge');
      } else if (isApiProblem(error) && error.query) {
        this.errorKey.set('exports.queryInvalid');
      } else {
        this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      }
    } finally {
      this.running.set(false);
    }
  }
}
