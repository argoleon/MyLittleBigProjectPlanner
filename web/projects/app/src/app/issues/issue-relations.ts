import { Component, computed, inject, input, output, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  Issue,
  IssueLink,
  IssueRelated,
  IssueRelation,
  StatusChip,
  deleteIssueLink,
  errorTranslationKey,
  isApiProblem,
} from 'ui';

import { chipCategory } from './issue-fields';
import { IssueLinkDialog } from './issue-link-dialog';

const ICONS: Readonly<Record<IssueRelation, string>> = {
  blocks: 'block',
  blocked_by: 'block',
  depends_on: 'link',
  required_by: 'link',
  duplicates: 'content_copy',
  duplicated_by: 'content_copy',
  relates_to: 'swap_horiz',
};

/** A line of the card: a sub-task, or a link. */
export interface RelationLine {
  readonly id: string;
  /** `subtask`, or how the link reads from the ticket. */
  readonly relation: IssueRelation | 'subtask';
  readonly icon: string;
  readonly issue: IssueRelated;
  /** The link, to remove it; none for a sub-task. */
  readonly link: IssueLink | null;
}

/** The lines of the card for `issue`: its sub-tasks, then its links. */
export function relationLines(issue: Issue): RelationLine[] {
  const children = (issue.children ?? []).map((child) => ({
    id: child.id,
    relation: 'subtask' as const,
    icon: 'subdirectory_arrow_right',
    issue: child,
    link: null,
  }));
  const links = (issue.links ?? []).map((link) => ({
    id: link.id,
    relation: link.relation,
    icon: ICONS[link.relation],
    issue: link.issue,
    link,
  }));
  return [...children, ...links];
}

/**
 * "Sub-tasks and links" of a ticket (mockup 03, L05-US03): each line with
 * its relation, the other ticket's key (a link to it), summary, project
 * when another, and status; links added and removed by those who edit.
 */
@Component({
  selector: 'app-issue-relations',
  imports: [MatButtonModule, MatIconModule, RouterLink, StatusChip, TranslocoPipe],
  templateUrl: './issue-relations.html',
  styleUrl: './issue-relations.scss',
})
export class IssueRelations {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  readonly issue = input.required<Issue>();
  /** Links are added and removed (edit on the ticket's project). */
  readonly canEdit = input(false);
  /** A link was added or removed: the ticket is read again. */
  readonly changed = output();

  protected readonly lines = computed(() => relationLines(this.issue()));
  protected readonly chipCategory = chipCategory;
  protected readonly errorKey = signal<string | null>(null);

  protected add(): void {
    this.errorKey.set(null);
    this.dialog
      .open<IssueLinkDialog, Issue, IssueLink>(IssueLinkDialog, { data: this.issue(), width: 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((link) => {
        if (link) this.changed.emit();
      });
  }

  protected remove(line: RelationLine): void {
    const link = line.link;
    if (!link) return;
    const data: ConfirmDialogData = {
      titleKey: 'issue.links.removeTitle',
      messageKey: 'issue.links.removeMessage',
      confirmKey: 'issue.links.remove',
      params: { key: line.issue.key },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.errorKey.set(null);
        try {
          await this.api.invoke(deleteIssueLink, { idOrKey: this.issue().id, linkId: link.id });
          this.changed.emit();
        } catch (error) {
          this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
        }
      });
  }
}
