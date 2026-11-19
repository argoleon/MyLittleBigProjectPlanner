import { Component, inject } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule } from '@angular/material/dialog';
import { TranslocoPipe } from '@jsverse/transloco';
import { BulkAction } from 'ui';

import { BulkReport } from './bulk-report';

/** The report of a bulk action run within the request (L06-US08). */
@Component({
  selector: 'app-bulk-report-dialog',
  imports: [BulkReport, MatButtonModule, MatDialogModule, TranslocoPipe],
  template: `
    <h2 mat-dialog-title>{{ 'bulk.report.title.' + report.kind | transloco }}</h2>
    <mat-dialog-content>
      <app-bulk-report [report]="report" />
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton="filled" type="button" mat-dialog-close>{{ 'dialog.close' | transloco }}</button>
    </mat-dialog-actions>
  `,
})
export class BulkReportDialog {
  protected readonly report = inject<BulkAction>(MAT_DIALOG_DATA);
}
