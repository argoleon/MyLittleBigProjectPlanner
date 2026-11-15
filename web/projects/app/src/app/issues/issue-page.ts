import { Component, computed, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTabsModule } from '@angular/material/tabs';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  ErrorState,
  InstantPipe,
  Issue,
  IssueFieldValue,
  LocalDatePipe,
  LocalNumberPipe,
  Priority,
  RichText,
  TypeChip,
  getIssue,
  getIssueForm,
  listMyNodePermissions,
} from 'ui';

import type { EditOutcome } from './issue-edit-dialog';
import { IssueActivity } from './issue-activity';
import { IssueAttachments } from './issue-attachments';
import { IssueComments } from './issue-comments';
import { createIssueFlow } from './issue-flow';
import { IssueParentDialog, ParentOutcome } from './issue-parent-dialog';
import { IssueRelations } from './issue-relations';
import { IssueStatus } from './issue-status';
import { IssueWatch } from './issue-watch';
import type { MoveOutcome } from './issue-move-dialog';
import { chipCategory, chipKind } from './issue-fields';

/**
 * A ticket (mockup 03, L05-US01): its type and key, summary, status,
 * description, details and custom fields, and the project it belongs to.
 * Those who create tickets in the project add sub-tasks from here. Its
 * history and comments share a card, in tabs (L05-US06).
 */
@Component({
  selector: 'app-issue-page',
  imports: [
    ErrorState,
    InstantPipe,
    IssueActivity,
    IssueAttachments,
    IssueComments,
    IssueRelations,
    IssueStatus,
    IssueWatch,
    LocalDatePipe,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTabsModule,
    Priority,
    RichText,
    RouterLink,
    TranslocoPipe,
    TypeChip,
  ],
  templateUrl: './issue-page.html',
  styleUrl: './issue-page.scss',
})
export class IssuePage {
  /** Key (or id) of the ticket, from the route. */
  readonly key = input.required<string>();

  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);

  protected readonly chipCategory = chipCategory;
  /** The number of comments, for their tab's label. */
  protected readonly commentCount = signal(0);
  /** Bumped when a comment changes, for the history to read it. */
  protected readonly activityRevision = signal(0);
  protected readonly bump = (n: number) => n + 1;
  protected readonly chipKind = chipKind;

  protected readonly issue = resource({
    params: () => this.key(),
    loader: ({ params }) => this.api.invoke(getIssue, { idOrKey: params, expand: 'parent,children,links' }),
  });
  private readonly projectId = computed(() => (this.issue.hasValue() ? this.issue.value().project.id : undefined));
  private readonly permissions = resource({
    params: () => this.projectId(),
    loader: ({ params }) => this.api.invoke(listMyNodePermissions, { id: params }),
  });
  /** Tenant admins, and those with `create_ticket` on the project, create tickets there. */
  private readonly canCreate = computed(
    () =>
      this.auth.session()?.account.tenantRole === 'tenant_admin' ||
      (this.permissions.hasValue() && this.permissions.value().includes('create_ticket')),
  );
  /** Tenant admins, and those with `edit` on the project, edit and move its tickets. */
  protected readonly canEdit = computed(
    () =>
      this.auth.session()?.account.tenantRole === 'tenant_admin' ||
      (this.permissions.hasValue() && this.permissions.value().includes('edit')),
  );
  /** Tenant admins, and those with `comment` on the project, write comments. */
  protected readonly canComment = computed(
    () =>
      this.auth.session()?.account.tenantRole === 'tenant_admin' ||
      (this.permissions.hasValue() && this.permissions.value().includes('comment')),
  );
  /** The types of the project, read only when the member may create. */
  private readonly form = resource({
    params: () => (this.canCreate() && this.issue.hasValue() ? this.issue.value().project.key : undefined),
    loader: ({ params }) => this.api.invoke(getIssueForm, { key: params }),
  });
  /** A type of the configuration may sit under this ticket's. */
  protected readonly canAddSubtask = computed(() => {
    if (!this.issue.hasValue() || !this.form.hasValue()) return false;
    const typeId = this.issue.value().type.id;
    return this.form.value().types.some((t) => t.parentIds.includes(typeId));
  });

  /** A custom field value as it reads: its display, else its value formatted by kind. */
  protected kindOf(value: IssueFieldValue): 'text' | 'number' | 'date' | 'instant' | 'boolean' | 'list' {
    if (value.display != null) return 'text';
    switch (value.kind) {
      case 'number':
        return 'number';
      case 'date':
        return 'date';
      case 'date_time':
        return 'instant';
      case 'checkbox':
        return 'boolean';
      case 'labels':
        return 'list';
      default:
        return 'text';
    }
  }

  protected textOf(value: IssueFieldValue): string {
    if (value.display != null) return value.display;
    return Array.isArray(value.value) ? (value.value as unknown[]).join(', ') : String(value.value);
  }

  protected async edit(issue: Issue): Promise<void> {
    const { IssueEditDialog } = await import('./issue-edit-dialog');
    this.dialog
      .open(IssueEditDialog, { data: issue, width: 'min(720px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((outcome?: EditOutcome) => {
        // An edit leaves the parent, sub-tasks and links as they were: kept from the ticket shown.
        if (outcome && 'saved' in outcome) {
          this.keepRelations(outcome.saved, issue);
        } else if (outcome) {
          this.issue.reload();
        }
      });
  }

  /**
   * Shows `saved` (an edit or a transition) with the parent, sub-tasks and
   * links of the ticket shown, which they leave as they were.
   */
  protected keepRelations(saved: Issue, shown: Issue): void {
    this.issue.set({ ...saved, parent: shown.parent, children: shown.children, links: shown.links });
  }

  protected async move(issue: Issue): Promise<void> {
    const { IssueMoveDialog } = await import('./issue-move-dialog');
    this.dialog
      .open(IssueMoveDialog, { data: issue, width: 'min(640px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((outcome?: MoveOutcome) => {
        if (outcome && 'moved' in outcome) void this.router.navigate(['/issues', outcome.moved.key]);
        else if (outcome) this.issue.reload();
      });
  }

  protected changeParent(issue: Issue): void {
    this.dialog
      .open<IssueParentDialog, Issue, ParentOutcome>(IssueParentDialog, { data: issue, width: 'min(560px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((outcome) => {
        // The page reads the ticket again: its parent, children and links with it.
        if (outcome) this.issue.reload();
      });
  }

  protected addSubtask(issue: Issue): void {
    void createIssueFlow(this.dialog, this.router, {
      parent: { id: issue.id, key: issue.key, typeId: issue.type.id, projectKey: issue.project.key },
    });
  }
}
