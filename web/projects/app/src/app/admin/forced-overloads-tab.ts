import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  EmptyState,
  ErrorState,
  ForcedOverload,
  InstantPipe,
  LocaleService,
  errorTranslationKey,
  isApiProblem,
  listForcedOverloads,
} from 'ui';

import { overloadDaysText } from '../issues/overload-text';

/** Forced overloads read per page. */
export const PAGE_SIZE = 50;

/**
 * "Forced overloads" tab of the administration (specification §8,
 * L08-US03): who wrote an assignment beyond its resource's capacity plus
 * tolerance in blocking mode, when, on which ticket and resource, and the
 * days overloaded at the time; newest first, page by page. Tenant admins
 * only. Each one is also in its ticket's history.
 */
@Component({
  selector: 'app-forced-overloads-tab',
  imports: [EmptyState, ErrorState, InstantPipe, MatButtonModule, MatProgressBarModule, MatTableModule, RouterLink, TranslocoPipe],
  templateUrl: './forced-overloads-tab.html',
  styleUrl: './forced-overloads-tab.scss',
})
export class ForcedOverloadsTab {
  private readonly api = inject(Api);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);

  protected readonly columns = ['when', 'issue', 'resource', 'actor', 'days'];
  protected readonly first = resource({ loader: () => this.api.invoke(listForcedOverloads, { limit: PAGE_SIZE }) });
  /** The pages read after the first one. */
  private readonly more = signal<readonly ForcedOverload[]>([]);
  private readonly cursor = signal<string | null | undefined>(undefined);
  protected readonly loadingMore = signal(false);
  protected readonly moreErrorKey = signal<string | null>(null);

  protected readonly items = computed<readonly ForcedOverload[]>(() => [...(this.first.hasValue() ? this.first.value().items : []), ...this.more()]);
  /** The cursor of the next page; null on the last one. */
  protected readonly next = computed(() => {
    const read = this.cursor();
    if (read !== undefined) return read;
    return this.first.hasValue() ? this.first.value().nextCursor : null;
  });

  protected daysText(forced: ForcedOverload): string {
    return overloadDaysText(forced.days, this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params));
  }

  protected async loadMore(): Promise<void> {
    const cursor = this.next();
    if (!cursor) return;
    this.loadingMore.set(true);
    this.moreErrorKey.set(null);
    try {
      const page = await this.api.invoke(listForcedOverloads, { limit: PAGE_SIZE, cursor });
      this.more.update((items) => [...items, ...page.items]);
      this.cursor.set(page.nextCursor);
    } catch (error) {
      this.moreErrorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.loadingMore.set(false);
    }
  }
}
