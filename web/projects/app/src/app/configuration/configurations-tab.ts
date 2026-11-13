import { Component, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  EmptyState,
  ErrorState,
  LocalNumberPipe,
  ProjectConfig,
  TranslatedPipe,
  deleteProjectConfig,
  errorTranslationKey,
  isApiProblem,
  listProjectConfigs,
} from 'ui';

import { ConfigChips } from './config-chips';
import { ConfigDialog, ConfigDialogData } from './config-dialog';

/**
 * "Project configurations" tab of the administration (mockup 11): every
 * configuration of the tenant, the default one first, with the projects
 * sharing it and its draft. Tenant admins and configuration admins.
 */
@Component({
  selector: 'app-configurations-tab',
  imports: [
    ConfigChips,
    EmptyState,
    ErrorState,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    RouterLink,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './configurations-tab.scss',
  templateUrl: './configurations-tab.html',
})
export class ConfigurationsTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);

  protected readonly columns = ['name', 'state', 'actions'];
  protected readonly configs = resource({
    // Configurations are few: every page at once.
    loader: async () => {
      const all: ProjectConfig[] = [];
      let cursor: string | undefined;
      do {
        const page = await this.api.invoke(listProjectConfigs, { limit: 200, cursor });
        all.push(...page.items);
        cursor = page.nextCursor ?? undefined;
      } while (cursor);
      return all;
    },
  });
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);

  /** Only an unused configuration, other than the default, can go. */
  protected deletable(config: ProjectConfig): boolean {
    return !config.isDefault && config.projectCount === 0;
  }

  protected create(): void {
    this.dialog
      .open<ConfigDialog, ConfigDialogData, ProjectConfig>(ConfigDialog, {
        data: { mode: 'create' },
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((created) => {
        if (created) void this.router.navigate(['/admin/configurations', created.id]);
      });
  }

  protected remove(config: ProjectConfig): void {
    const params = { name: config.name ?? '' };
    const data: ConfirmDialogData = {
      titleKey: 'config.delete.title',
      messageKey: 'config.delete.message',
      confirmKey: 'config.delete.confirm',
      params,
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteProjectConfig, { id: config.id, 'If-Match': `"${config.version}"` });
          this.notice.set({ ok: true, key: 'config.delete.done', params });
          this.configs.reload();
        } catch (error) {
          this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }
}
