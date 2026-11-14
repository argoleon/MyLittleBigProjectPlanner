import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  EmptyState,
  ErrorState,
  InstantPipe,
  IssueSummary,
  Priority,
  StatusChip,
  TypeChip,
  errorTranslationKey,
  isApiProblem,
  listMyIssues,
} from 'ui';

import { createIssueFlow } from './issue-flow';
import { chipCategory, chipKind } from './issue-fields';

/** Tickets per page. */
const PAGE_SIZE = 50;

/**
 * "My tickets" (L05-US01): those assigned to the member or reported by
 * them, newest first, a page at a time; a ticket is created from here in
 * a project chosen in the dialog.
 */
@Component({
  selector: 'app-my-tickets-page',
  imports: [
    EmptyState,
    ErrorState,
    InstantPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    Priority,
    RouterLink,
    StatusChip,
    TranslocoPipe,
    TypeChip,
  ],
  templateUrl: './my-tickets-page.html',
  styleUrl: './my-tickets-page.scss',
})
export class MyTicketsPage {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);

  protected readonly columns = ['ticket', 'summary', 'project', 'status', 'priority', 'updated'];
  protected readonly chipCategory = chipCategory;
  protected readonly chipKind = chipKind;

  protected readonly first = resource({
    loader: () => this.api.invoke(listMyIssues, { limit: PAGE_SIZE }),
  });
  /** Pages loaded after the first one, and the cursor of the next. */
  private readonly more = signal<readonly IssueSummary[]>([]);
  private readonly moreCursor = signal<string | null | undefined>(undefined);
  protected readonly loadingMore = signal(false);
  protected readonly moreError = signal<string | null>(null);

  protected readonly rows = computed(() => (this.first.hasValue() ? [...this.first.value().items, ...this.more()] : []));
  protected readonly nextCursor = computed(() => {
    const more = this.moreCursor();
    if (more !== undefined) return more;
    return this.first.hasValue() ? this.first.value().nextCursor : null;
  });
  protected readonly trackRow = (_: number, row: IssueSummary) => row.id;

  protected reload(): void {
    this.more.set([]);
    this.moreCursor.set(undefined);
    this.first.reload();
  }

  protected async loadMore(): Promise<void> {
    const cursor = this.nextCursor();
    if (!cursor) return;
    this.loadingMore.set(true);
    this.moreError.set(null);
    try {
      const page = await this.api.invoke(listMyIssues, { limit: PAGE_SIZE, cursor });
      this.more.update((more) => [...more, ...page.items]);
      this.moreCursor.set(page.nextCursor ?? null);
    } catch (error) {
      this.moreError.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.loadingMore.set(false);
    }
  }

  protected create(): void {
    void createIssueFlow(this.dialog, this.router, {});
  }
}
