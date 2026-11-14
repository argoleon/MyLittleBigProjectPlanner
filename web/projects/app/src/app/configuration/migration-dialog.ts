import { Component, DestroyRef, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigProjectItem,
  ErrorState,
  LocalNumberPipe,
  Migration,
  ProjectConfig,
  errorTranslationKey,
  getMigration,
  getMigrationPlan,
  getProjectConfig,
  isApiProblem,
  startMigration,
} from 'ui';

import { isVeiled } from '../hierarchy/veiled';
import { initialChoices, isComplete, mappings, ticketTotal } from './migration';

/** The configuration to migrate, and the projects it affects. */
export interface MigrationDialogData {
  readonly config: ProjectConfig;
  readonly projects: readonly ConfigProjectItem[];
}

/** How often the progress is read, in milliseconds. */
export const PROGRESS_INTERVAL = 800;

/**
 * The status and type mapping wizard (mockup 13, L04-US08): each removed
 * status and type with its tickets and a target to choose (the new ones
 * marked, a suggestion chosen), the projects affected; once every one is
 * mapped, the migration starts in the background and its progress shows
 * until the draft is published. Closes with the configuration published.
 */
@Component({
  selector: 'app-migration-dialog',
  imports: [
    ErrorState,
    LocalNumberPipe,
    MatButtonModule,
    MatDialogModule,
    MatFormFieldModule,
    MatIconModule,
    MatProgressBarModule,
    MatSelectModule,
    TranslocoPipe,
  ],
  styleUrl: './migration-dialog.scss',
  templateUrl: './migration-dialog.html',
})
export class MigrationDialog {
  protected readonly data = inject<MigrationDialogData>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<MigrationDialog, ProjectConfig>>(MatDialogRef);
  private closed = false;

  protected readonly plan = resource({
    loader: async () => {
      const plan = await this.api.invoke(getMigrationPlan, { id: this.data.config.id });
      this.choices.set(initialChoices(plan));
      return plan;
    },
  });
  protected readonly choices = signal<Record<string, string | null>>({});
  protected readonly complete = computed(() => this.plan.hasValue() && isComplete(this.plan.value(), this.choices()));
  protected readonly total = computed(() => (this.plan.hasValue() ? ticketTotal(this.plan.value()) : 0));
  protected readonly migration = signal<Migration | null>(null);
  protected readonly starting = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  constructor() {
    inject(DestroyRef).onDestroy(() => (this.closed = true));
  }

  /** The key of a project, or null when it shows veiled. */
  protected keyOf(item: ConfigProjectItem): string | null {
    return isVeiled(item) ? null : item.key;
  }

  protected choose(id: string, target: string): void {
    this.choices.set({ ...this.choices(), [id]: target });
  }

  protected async start(): Promise<void> {
    if (!this.plan.hasValue() || !this.complete()) return;
    const plan = this.plan.value();
    this.starting.set(true);
    this.errorKey.set(null);
    try {
      const migration = await this.api.invoke(startMigration, {
        id: this.data.config.id,
        'If-Match': `"${plan.version}"`,
        body: mappings(plan, this.choices()),
      });
      this.migration.set(migration);
      await this.follow(migration);
    } catch (error) {
      if (isApiProblem(error) && error.status === 412) {
        // Changed meanwhile: the plan is read again.
        this.errorKey.set('config.migration.stale');
        this.plan.reload();
      } else {
        this.errorKey.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
      }
    } finally {
      this.starting.set(false);
    }
  }

  /** Reads the progress until the migration is over; closes once published. */
  private async follow(started: Migration): Promise<void> {
    let migration = started;
    while (!this.closed && (migration.state === 'pending' || migration.state === 'running')) {
      migration = await this.api.invoke(getMigration, { id: this.data.config.id, migrationId: started.id });
      this.migration.set(migration);
      if (migration.state === 'pending' || migration.state === 'running') {
        await new Promise((resolve) => setTimeout(resolve, PROGRESS_INTERVAL));
      }
    }
    if (migration.state === 'done' && !this.closed) {
      this.dialogRef.close(await this.api.invoke(getProjectConfig, { id: this.data.config.id }));
    }
  }
}
