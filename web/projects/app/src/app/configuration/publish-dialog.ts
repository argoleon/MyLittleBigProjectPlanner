import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MAT_DIALOG_DATA, MatDialogModule, MatDialogRef } from '@angular/material/dialog';
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

/**
 * Publishing the draft of a configuration (L04-US01): first the projects it
 * affects, which the user confirms; then the publication, with the version
 * the projects were read at. Closes with the configuration published.
 */
@Component({
  selector: 'app-publish-dialog',
  imports: [ErrorState, LocalNumberPipe, MatButtonModule, MatDialogModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  styleUrl: './publish-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'config.publishDialog.title' | transloco: { name: config.name ?? '' } }}</h2>
    <mat-dialog-content>
      @if (errorKey(); as key) {
        <p class="message error" role="alert">{{ key | transloco }}</p>
      }
      @if (impact.isLoading()) {
        <mat-progress-bar mode="indeterminate" [attr.aria-label]="'state.loading' | transloco" />
      } @else if (impact.error()) {
        <ui-error-state [error]="impact.error()" [headingLevel]="3" (retry)="impact.reload()" />
      } @else if (impact.value(); as impact) {
        @if (impact.projects.length === 0) {
          <p class="intro">{{ 'config.publishDialog.none' | transloco }}</p>
        } @else {
          <p class="intro">{{ introKey() | transloco: { count: (impact.projects.length | localNumber) } }}</p>
          <ul class="projects" [attr.aria-label]="'config.publishDialog.projects' | transloco">
            @for (project of impact.projects; track project.id) {
              <li>
                @if (asProject(project); as seen) {
                  <span class="name">{{ seen.name }}</span> <span class="key">{{ seen.key }}</span>
                  @if (seen.archived) {
                    <span class="archived">{{ 'config.projects.archived' | transloco }}</span>
                  }
                } @else {
                  <mat-icon class="lock" aria-hidden="true">lock</mat-icon>
                  <span class="veiled">{{ 'config.projects.veiled' | transloco }}</span>
                }
              </li>
            }
          </ul>
        }
        @if (impact.needsMigration) {
          <p class="message error" role="alert">{{ 'config.publishDialog.migration' | transloco }}</p>
        }
      }
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'dialog.cancel' | transloco }}</button>
      <button matButton="filled" type="button" class="confirm" [disabled]="!ready() || publishing()" (click)="publish()">
        {{ 'config.publishDialog.confirm' | transloco }}
      </button>
    </mat-dialog-actions>
  `,
})
export class PublishDialog {
  protected readonly config = inject<ProjectConfig>(MAT_DIALOG_DATA);
  private readonly api = inject(Api);
  private readonly dialogRef = inject<MatDialogRef<PublishDialog, ProjectConfig>>(MatDialogRef);

  protected readonly impact = resource({
    loader: () => this.api.invoke(getProjectConfigPublishImpact, { id: this.config.id }),
  });
  protected readonly introKey = computed(() =>
    countKey('config.publishDialog.intro.', this.impact.hasValue() ? this.impact.value().projects.length : 0),
  );
  /** Read, and nothing to migrate first. */
  protected readonly ready = computed(() => this.impact.hasValue() && !this.impact.value().needsMigration);
  protected readonly publishing = signal(false);
  protected readonly errorKey = signal<string | null>(null);

  /** The project of a row, or null when it shows veiled. */
  protected asProject(item: ConfigProjectItem) {
    return isVeiled(item) ? null : item;
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
