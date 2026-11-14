import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialog, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigProjectItem,
  ErrorState,
  LocalNumberPipe,
  ProjectConfig,
  errorTranslationKey,
  getProjectConfigPublishImpact,
  isApiProblem,
  publishProjectConfig,
} from 'ui';

import { isVeiled } from '../hierarchy/veiled';
import { countKey } from './config-chips';
import { MigrationDialog, MigrationDialogData } from './migration-dialog';

/**
 * Publishing the draft of a configuration (L04-US01): first the projects it
 * affects, which the user confirms; then the publication, with the version
 * the projects were read at. Closes with the configuration published.
 */
@Component({
  selector: 'app-publish-dialog',
  imports: [ErrorState, LocalNumberPipe, MatButtonModule, MatDialogModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  styleUrl: './publish-dialog.scss',
  templateUrl: './publish-dialog.html',
})
export class PublishDialog {
  protected readonly config = inject<ProjectConfig>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<PublishDialog, ProjectConfig>>(MatDialogRef);
  private readonly dialog = inject(MatDialog);

  protected readonly impact = resource({
    loader: () => this.api.invoke(getProjectConfigPublishImpact, { id: this.config.id }),
  });
  protected readonly introKey = computed(() =>
    countKey('config.publishDialog.intro.', this.impact.hasValue() ? this.impact.value().projects.length : 0),
  );
  /** Read, nothing to migrate first, and every workflow ready. */
  protected readonly ready = computed(
    () => this.impact.hasValue() && !this.impact.value().needsMigration && this.impact.value().workflowProblems.length === 0,
  );
  /** Tickets use what the draft removes: they are mapped and migrated first (L04-US08). */
  protected readonly toMigrate = computed(
    () => this.impact.hasValue() && this.impact.value().needsMigration && this.impact.value().workflowProblems.length === 0,
  );
  protected readonly publishing = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  protected ticketsKey(count: number): string {
    return countKey('config.publishDialog.tickets.', count);
  }

  /** The project of a row, or null when it shows veiled. */
  protected asProject(item: ConfigProjectItem) {
    return isVeiled(item) ? null : item;
  }

  /** Opens the mapping wizard; closes with the configuration once migrated and published. */
  protected migrate(): void {
    if (!this.impact.hasValue()) return;
    const data: MigrationDialogData = { config: this.config, projects: this.impact.value().projects };
    this.dialog
      .open<MigrationDialog, MigrationDialogData, ProjectConfig>(MigrationDialog, { data, width: 'min(760px, 100vw)' })
      .afterClosed()
      .subscribe((published) => {
        if (published) this.dialogRef.close(published);
      });
  }

  protected async publish(): Promise<void> {
    if (!this.impact.hasValue()) return;
    this.publishing.set(true);
    this.errorKey.set(null);
    try {
      const published = await this.api.invoke(publishProjectConfig, {
        id: this.config.id,
        'If-Match': `"${this.impact.value().version}"`,
      });
      this.dialogRef.close(published);
    } catch (error) {
      if (isApiProblem(error) && error.status === 412) {
        // Changed meanwhile: the projects are read again before a new confirmation.
        this.errorKey.set('config.publishDialog.stale');
        this.impact.reload();
      } else {
        this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      }
    } finally {
      this.publishing.set(false);
    }
  }
}
