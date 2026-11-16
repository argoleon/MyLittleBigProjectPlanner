import { Component, computed, inject, input, linkedSignal, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import { ActivityEntry, Api, ErrorState, InstantPipe, Issue, LocaleService, RichText, listIssueActivity } from 'ui';

import { sentenceOf } from './activity-sentence';

/** Entries read at a time. */
export const ACTIVITY_PAGE = 20;

/** The initials of a name, for its avatar. */
function initials(name: string): string {
  const words = name.split(/\s+/).filter((w) => /^\p{L}/u.test(w));
  return words
    .slice(0, 2)
    .map((w) => w[0].toUpperCase())
    .join('');
}

/**
 * The history of a ticket (mockup 03, L05-US06): who changed what and
 * when, newest first, as sentences in the reader's language, with the
 * changes made through MCP or the API marked; a rich text's before and
 * after on demand, and a deleted comment's content for moderators.
 */
@Component({
  selector: 'app-issue-activity',
  imports: [ErrorState, InstantPipe, MatButtonModule, MatIconModule, MatProgressBarModule, RichText, TranslocoPipe],
  templateUrl: './issue-activity.html',
  styleUrl: './issue-activity.scss',
})
export class IssueActivity {
  private readonly api = inject(Api);
  private readonly transloco = inject(TranslocoService);
  private readonly locale = inject(LocaleService);
  private readonly language = toSignal(this.transloco.langChanges$, { initialValue: this.transloco.getActiveLang() });

  readonly issue = input.required<Issue>();
  /** Bumped when something the ticket does not show changed (a comment). */
  readonly revision = input(0);

  /** The newest page, read again whenever the ticket or its comments change. */
  protected readonly newest = resource({
    params: () => ({ issue: this.issue(), revision: this.revision() }),
    loader: ({ params }) => this.api.invoke(listIssueActivity, { idOrKey: params.issue.id, limit: ACTIVITY_PAGE }),
  });
  /** The older pages read since. */
  private readonly older = linkedSignal<unknown, ActivityEntry[]>({ source: () => this.newest.value(), computation: () => [] });
  private readonly cursor = linkedSignal<unknown, string | null>({
    source: () => this.newest.value(),
    computation: () => (this.newest.hasValue() ? this.newest.value().nextCursor : null),
  });
  protected readonly hasMore = computed(() => this.cursor() !== null);
  protected readonly loadingMore = signal(false);
  protected readonly moreError = signal<unknown>(null);

  /** The entries shown, each with its sentence. */
  protected readonly entries = computed(() => {
    if (!this.newest.hasValue()) return [];
    this.language();
    const context = {
      translate: (key: string, params?: Record<string, unknown>) => this.transloco.translate(key, params),
      locale: this.locale.formattingLocale(),
      timeZone: this.locale.timeZone(),
    };
    return [...this.newest.value().items, ...this.older()].map((entry) => {
      const sentence = sentenceOf(entry, context);
      return { entry, sentence, initials: initials(sentence.who) };
    });
  });

  /** The entries whose before and after, or deleted content, are open. */
  protected readonly open = signal<ReadonlySet<string>>(new Set());

  protected toggle(entry: ActivityEntry): void {
    this.open.update((open) => {
      const next = new Set(open);
      if (!next.delete(entry.id)) next.add(entry.id);
      return next;
    });
  }

  protected async more(): Promise<void> {
    const cursor = this.cursor();
    if (!cursor) return;
    this.loadingMore.set(true);
    this.moreError.set(null);
    try {
      const page = await this.api.invoke(listIssueActivity, { idOrKey: this.issue().id, limit: ACTIVITY_PAGE, cursor });
      this.older.update((older) => [...older, ...page.items]);
      this.cursor.set(page.nextCursor);
    } catch (error) {
      this.moreError.set(error);
    } finally {
      this.loadingMore.set(false);
    }
  }
}
