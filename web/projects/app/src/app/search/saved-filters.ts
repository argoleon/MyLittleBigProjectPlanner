import { Component, computed, inject, input, output, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  SavedFilter,
  deleteSavedFilter,
  errorTranslationKey,
  isApiProblem,
  listSavedFilters,
} from 'ui';

/**
 * The saved filters of the search screen (mockup 15, L06-US04): the member's
 * own, then those shared with them; each one opens the search
 * (`?filter=`). Its menu changes the member's filters, and deletes those the
 * member may delete (their own, any shared one for a tenant admin).
 */
@Component({
  selector: 'app-saved-filters',
  imports: [MatButtonModule, MatIconModule, MatMenuModule, MatProgressBarModule, RouterLink, TranslocoPipe],
  templateUrl: './saved-filters.html',
  styleUrl: './saved-filters.scss',
})
export class SavedFilters {
  /** The filter open in the search, if any. */
  readonly active = input<string | null>(null);
  /** The member asks to change one of their filters. */
  readonly edit = output<SavedFilter>();
  /** A filter was deleted. */
  readonly deleted = output<SavedFilter>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  protected readonly list = resource({ loader: () => this.api.invoke(listSavedFilters, {}) });
  private readonly items = computed(() => (this.list.hasValue() ? this.list.value().items : []));
  protected readonly mine = computed(() => this.items().filter((f) => f.mine));
  protected readonly shared = computed(() => this.items().filter((f) => !f.mine));
  /** Why the last deletion failed. */
  protected readonly errorKey = signal<string | null>(null);

  /** Reads the filters again (one was saved). */
  reload(): void {
    this.list.reload();
  }

  protected remove(filter: SavedFilter): void {
    const data: ConfirmDialogData = {
      titleKey: 'search.filters.delete.title',
      messageKey: filter.mine ? 'search.filters.delete.message' : 'search.filters.delete.messageShared',
      confirmKey: 'search.filters.delete.confirm',
      params: { name: filter.name },
    };
    this.dialog
      .open(ConfirmDialog, { data })
      .afterClosed()
      .subscribe((yes) => {
        if (yes) void this.delete(filter);
      });
  }

  private async delete(filter: SavedFilter): Promise<void> {
    this.errorKey.set(null);
    try {
      await this.api.invoke(deleteSavedFilter, { id: filter.id, 'If-Match': `"${filter.version}"` });
      this.list.reload();
      this.deleted.emit(filter);
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      this.list.reload();
    }
  }
}
