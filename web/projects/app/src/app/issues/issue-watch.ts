import { Component, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Issue, IssueWatchers, errorTranslationKey, getIssueWatchers, isApiProblem, unwatchIssue, watchIssue } from 'ui';

/**
 * Watching a ticket (L05-US07): whether the member watches it and how many
 * do; a toggle to start or stop. Watchers are notified as the
 * configuration's rules say.
 */
@Component({
  selector: 'app-issue-watch',
  imports: [MatButtonModule, MatIconModule, TranslocoPipe],
  template: `
    @if (watchers.hasValue()) {
      <button
        matButton="outlined"
        type="button"
        class="watch"
        [attr.aria-pressed]="watchers.value().watching"
        [disabled]="saving()"
        (click)="toggle(watchers.value())"
      >
        <mat-icon aria-hidden="true">{{ watchers.value().watching ? 'visibility' : 'visibility_off' }}</mat-icon>
        {{ (watchers.value().watching ? 'issue.watch.watching' : 'issue.watch.watch') | transloco: { count: watchers.value().count } }}
      </button>
    }
    @if (errorKey(); as key) {
      <span class="error" role="alert">{{ key | transloco }}</span>
    }
  `,
  styleUrl: './issue-watch.scss',
})
export class IssueWatch {
  private readonly api = inject(Api);

  readonly issue = input.required<Issue>();
  /** Bumped when the ticket changed elsewhere: read again. */
  readonly refresh = input(0);

  protected readonly watchers = resource({
    // Read again when the ticket is (an assignee starts watching it).
    params: () => ({ id: this.issue().id, version: this.issue().version, refresh: this.refresh() }),
    loader: ({ params }) => this.api.invoke(getIssueWatchers, { idOrKey: params.id }),
  });
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected async toggle(current: IssueWatchers): Promise<void> {
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      const fn = current.watching ? unwatchIssue : watchIssue;
      this.watchers.set(await this.api.invoke(fn, { idOrKey: this.issue().id }));
    } catch (error) {
      this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.saving.set(false);
    }
  }
}
