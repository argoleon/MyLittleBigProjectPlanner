import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Issue, IssueRelated, errorTranslationKey, fieldErrorTranslationKey, isApiProblem, setIssueParent } from 'ui';

import { IssuePicker } from './issue-picker';

/** How the parent dialog closes: the ticket saved, or a reload asked for after a conflict. */
export type ParentOutcome = { readonly saved: Issue } | { readonly reload: true };

/**
 * Puts a ticket under another of its project, or at the top (L05-US03):
 * only tickets of a type its type may sit under are offered, never the
 * ticket itself or its sub-tasks; a sub-task always keeps a parent.
 */
@Component({
  selector: 'app-issue-parent-dialog',
  imports: [FormsModule, IssuePicker, MatButtonModule, MatCheckboxModule, MatDialogModule, TranslocoPipe],
  template: `
    <h2 mat-dialog-title>{{ 'issue.parent.title' | transloco: { key: ticket.key } }}</h2>
    <mat-dialog-content class="fields">
      @if (conflict()) {
        <div class="message error conflict" role="alert">
          <p>{{ 'issue.edit.conflict' | transloco }}</p>
          <button matButton="tonal" type="button" class="reload" (click)="reload()">{{ 'issue.edit.reload' | transloco }}</button>
        </div>
      }
      @if (errorKey(); as key) {
        <p class="message error" role="alert">{{ key | transloco }}</p>
      }
      @if (!ticket.type.isSubtask) {
        <mat-checkbox class="top" [(ngModel)]="atTop">{{ 'issue.parent.top' | transloco }}</mat-checkbox>
      }
      @if (!atTop) {
        <app-issue-picker
          [label]="'issue.parent.parent' | transloco"
          [projectKey]="ticket.project.key"
          [accept]="allowed"
          [error]="parentError()"
          [(selected)]="parent"
        />
      }
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
      <button matButton="filled" type="button" class="confirm" [disabled]="saving() || conflict() || (!atTop && !parent())" (click)="save()">
        {{ 'issue.edit.save' | transloco }}
      </button>
    </mat-dialog-actions>
  `,
  styleUrl: './issue-dialog.scss',
})
export class IssueParentDialog {
  protected readonly ticket = inject<Issue>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<IssueParentDialog, ParentOutcome>>(MatDialogRef);

  protected atTop = false;
  protected readonly parent = signal<IssueRelated | null>(null);
  /** Of a type the ticket's may sit under; not itself, nor one of its sub-tasks. */
  protected readonly allowed = (issue: IssueRelated) =>
    issue.id !== this.ticket.id &&
    issue.id !== this.ticket.parentId &&
    this.ticket.type.parentIds.includes(issue.type.id) &&
    !(this.ticket.children ?? []).some((child) => child.id === issue.id);
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly parentError = signal<string | null>(null);
  protected readonly conflict = signal(false);

  protected reload(): void {
    this.dialogRef.close({ reload: true });
  }

  protected async save(): Promise<void> {
    const parentId = this.atTop ? null : (this.parent()?.id ?? null);
    if (!this.atTop && !parentId) return;
    this.saving.set(true);
    this.errorKey.set(null);
    this.parentError.set(null);
    try {
      const saved = await this.api.invoke(setIssueParent, {
        idOrKey: this.ticket.id,
        'If-Match': `"${this.ticket.version}"`,
        body: { parentId },
      });
      this.dialogRef.close({ saved });
    } catch (error) {
      if (isApiProblem(error) && error.status === 412) {
        this.conflict.set(true);
        return;
      }
      const parentError = isApiProblem(error) ? error.fieldErrors.find((e) => e.field === 'parentId') : undefined;
      if (parentError) this.parentError.set(fieldErrorTranslationKey(parentError.code));
      else this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.saving.set(false);
    }
  }
}
