import { MatDialog } from '@angular/material/dialog';
import { Router } from '@angular/router';
import { Issue } from 'ui';

/** Where the new ticket goes. */
export interface IssueDialogData {
  /** The project, chosen beforehand (its page); otherwise picked in the dialog. */
  readonly projectKey?: string;
  /** The ticket it sits under (a sub-task), in the same project. */
  readonly parent?: { readonly id: string; readonly key: string; readonly typeId: string; readonly projectKey: string };
}

/**
 * Opens the creation dialog and, once the ticket is created, its page.
 * Resolves with the ticket, or undefined if cancelled. The dialog (and the
 * rich text editor with it) loads on first use only.
 */
export async function createIssueFlow(dialog: MatDialog, router: Router, data: IssueDialogData): Promise<Issue | undefined> {
  const { IssueDialog } = await import('./issue-dialog');
  return new Promise((resolve) => {
    dialog
      .open(IssueDialog, { data, width: 'min(720px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((issue?: Issue) => {
        if (issue) void router.navigate(['/issues', issue.key]);
        resolve(issue);
      });
  });
}
