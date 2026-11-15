import { Component, computed, effect, inject, input, output, resource, signal } from '@angular/core';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  CommentVisibility,
  ConfirmDialog,
  ConfirmDialogData,
  ErrorState,
  InstantPipe,
  Issue,
  IssueComment,
  RichText,
  createIssueComment,
  deleteIssueComment,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  listIssueComments,
  listProjectPeople,
  searchIssues,
  updateIssueComment,
} from 'ui';
import { RichTextDoc, RichTextEditor, Suggestion, SuggestionSource } from 'ui/rich-text';

/** The translation key of why a comment was refused. */
function refusalOf(error: unknown): string {
  if (!isApiProblem(error)) return errorTranslationKey('unknown');
  const field = error.fieldErrors.find((e) => e.field === 'content');
  return field ? fieldErrorTranslationKey(field.code) : error.translationKey;
}

/**
 * The comments of a ticket (mockup 03, L05-US04): oldest first, each with
 * its author, date, visibility and rendered content, a placeholder once
 * deleted; written, edited and deleted here, with `@` mentions and `#`
 * ticket references, internal or public. Shown in the Comments tab of the
 * ticket page, which tells their number (`countChange`).
 */
@Component({
  selector: 'app-issue-comments',
  imports: [
    ErrorState,
    InstantPipe,
    MatButtonModule,
    MatButtonToggleModule,
    MatIconModule,
    MatProgressBarModule,
    ReactiveFormsModule,
    RichText,
    RichTextEditor,
    TranslocoPipe,
  ],
  templateUrl: './issue-comments.html',
  styleUrl: './issue-comments.scss',
})
export class IssueComments {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  readonly issue = input.required<Issue>();
  /** The member may write comments here. */
  readonly canComment = input(false);

  protected readonly comments = resource({
    params: () => this.issue().id,
    loader: ({ params }) => this.api.invoke(listIssueComments, { idOrKey: params }),
  });
  private readonly count = computed(() => (this.comments.hasValue() ? this.comments.value().filter((c) => !c.deleted).length : 0));
  /** The number of comments not deleted, for the tab's label. */
  readonly countChange = output<number>();
  /** A comment was written, edited or deleted: the history changed. */
  readonly changed = output<void>();

  constructor() {
    effect(() => this.countChange.emit(this.count()));
  }

  /** The people `@` offers: members, from the ticket's project. */
  protected readonly people: SuggestionSource = async (query) =>
    (await this.api.invoke(listProjectPeople, { key: this.issue().project.key, search: query || undefined })).map(
      (person): Suggestion => ({ id: person.id, label: person.name }),
    );
  /** The tickets `#` offers, by key or summary. */
  protected readonly tickets: SuggestionSource = async (query) =>
    query.trim()
      ? (await this.api.invoke(searchIssues, { search: query })).map((issue): Suggestion => ({ id: issue.key, label: issue.key, detail: issue.summary }))
      : [];

  protected readonly content = new FormControl<RichTextDoc | null>(null);
  protected readonly visibility = signal<CommentVisibility>('internal');
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  /** The comment being edited, and what the edit holds. */
  protected readonly editing = signal<string | null>(null);
  protected readonly editContent = new FormControl<RichTextDoc | null>(null);
  protected readonly editVisibility = signal<CommentVisibility>('internal');
  protected readonly editError = signal<string | null>(null);

  protected async write(): Promise<void> {
    const content = this.content.value;
    if (!content) {
      this.errorKey.set('errors.field.required');
      return;
    }
    this.saving.set(true);
    this.errorKey.set(null);
    try {
      await this.api.invoke(createIssueComment, { idOrKey: this.issue().id, body: { content, visibility: this.visibility() } });
      this.content.reset(null);
      this.visibility.set('internal');
      this.comments.reload();
      this.changed.emit();
    } catch (error) {
      this.errorKey.set(refusalOf(error));
    } finally {
      this.saving.set(false);
    }
  }

  protected edit(comment: IssueComment): void {
    this.editContent.setValue(comment.content as RichTextDoc | null);
    this.editVisibility.set(comment.visibility);
    this.editError.set(null);
    this.editing.set(comment.id);
  }

  protected async save(comment: IssueComment): Promise<void> {
    const content = this.editContent.value;
    if (!content) {
      this.editError.set('errors.field.required');
      return;
    }
    this.saving.set(true);
    this.editError.set(null);
    try {
      await this.api.invoke(updateIssueComment, {
        idOrKey: this.issue().id,
        commentId: comment.id,
        'If-Match': `"${comment.version}"`,
        body: { content, visibility: this.editVisibility() },
      });
      this.editing.set(null);
      this.comments.reload();
      this.changed.emit();
    } catch (error) {
      this.editError.set(isApiProblem(error) && error.status === 412 ? 'issue.comments.conflict' : refusalOf(error));
    } finally {
      this.saving.set(false);
    }
  }

  protected remove(comment: IssueComment): void {
    const data: ConfirmDialogData = {
      titleKey: 'issue.comments.removeTitle',
      messageKey: 'issue.comments.removeMessage',
      confirmKey: 'issue.comments.remove',
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.errorKey.set(null);
        try {
          await this.api.invoke(deleteIssueComment, { idOrKey: this.issue().id, commentId: comment.id });
          this.comments.reload();
          this.changed.emit();
        } catch (error) {
          this.errorKey.set(refusalOf(error));
        }
      });
  }
}
