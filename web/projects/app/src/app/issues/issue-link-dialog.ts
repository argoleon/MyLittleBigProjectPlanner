import { Component, inject, signal } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ISSUE_RELATION, Issue, IssueLink, IssueRelated, IssueRelation, createIssueLink, errorTranslationKey, fieldErrorTranslationKey, isApiProblem } from 'ui';

import { IssuePicker } from './issue-picker';

/**
 * Links a ticket to another (L05-US03), in any project the member sees:
 * how the link reads from this ticket, then the other ticket, found by
 * key or summary. Closes with the link.
 */
@Component({
  selector: 'app-issue-link-dialog',
  imports: [FormsModule, IssuePicker, MatButtonModule, MatDialogModule, MatFormFieldModule, MatSelectModule, TranslocoPipe],
  template: `
    <h2 mat-dialog-title>{{ 'issue.links.addTitle' | transloco: { key: ticket.key } }}</h2>
    <mat-dialog-content class="fields">
      @if (errorKey(); as key) {
        <p class="message error" role="alert">{{ key | transloco }}</p>
      }
      <mat-form-field appearance="outline">
        <mat-label>{{ 'issue.links.relation' | transloco }}</mat-label>
        <mat-select [(ngModel)]="relation" [aria-label]="'issue.links.relation' | transloco">
          @for (option of relations; track option) {
            <mat-option [value]="option">{{ 'issue.links.relations.' + option | transloco }}</mat-option>
          }
        </mat-select>
      </mat-form-field>
      <app-issue-picker [label]="'issue.links.other' | transloco" [accept]="notItself" [error]="issueError()" [(selected)]="other" />
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
      <button matButton="filled" type="button" class="confirm" [disabled]="saving() || !other()" (click)="save()">
        {{ 'issue.links.add' | transloco }}
      </button>
    </mat-dialog-actions>
  `,
  styleUrl: './issue-dialog.scss',
})
export class IssueLinkDialog {
  protected readonly ticket = inject<Issue>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<IssueLinkDialog, IssueLink>>(MatDialogRef);

  protected readonly relations = ISSUE_RELATION;
  protected relation: IssueRelation = 'relates_to';
  protected readonly other = signal<IssueRelated | null>(null);
  protected readonly notItself = (issue: IssueRelated) => issue.id !== this.ticket.id;
  protected readonly saving = signal(false);
  protected readonly errorKey = signal<string | null>(null);
  protected readonly issueError = signal<string | null>(null);

  protected async save(): Promise<void> {
    const other = this.other();
    if (!other) return;
    this.saving.set(true);
    this.errorKey.set(null);
    this.issueError.set(null);
    try {
      const link = await this.api.invoke(createIssueLink, { idOrKey: this.ticket.id, body: { relation: this.relation, issue: other.id } });
      this.dialogRef.close(link);
    } catch (error) {
      const issueError = isApiProblem(error) ? error.fieldErrors.find((e) => e.field === 'issue') : undefined;
      if (issueError) this.issueError.set(fieldErrorTranslationKey(issueError.code));
      else this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.saving.set(false);
    }
  }
}
