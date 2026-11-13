import { Component, Injectable, computed, effect, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTabsModule } from '@angular/material/tabs';
import { Router, RouterLink, RouterLinkActive, RouterOutlet } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  ErrorState,
  LocalNumberPipe,
  ProjectConfig,
  TranslatedPipe,
  discardProjectConfigDraft,
  errorTranslationKey,
  getProjectConfig,
  isApiProblem,
} from 'ui';

import { ConfigChips, countKey } from './config-chips';
import { ConfigDialog, ConfigDialogData } from './config-dialog';
import { PublishDialog } from './publish-dialog';

/** The configuration shown, shared with its tabs. */
@Injectable()
export class ConfigContext {
  readonly config = signal<ProjectConfig | null>(null);
  /** A message for the user, from the page or a tab. */
  readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);
}

/** Sections of a configuration (mockup 11), each a routed tab. */
export const CONFIG_SECTIONS = ['types', 'fields', 'workflows', 'screens', 'notifications'] as const;

/**
 * A project configuration (mockup 11): its sharing and draft, Duplicate and
 * Publish, and its sections in routed tabs, the projects last.
 */
@Component({
  selector: 'app-config-page',
  imports: [
    ConfigChips,
    ErrorState,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTabsModule,
    RouterLink,
    RouterLinkActive,
    RouterOutlet,
    TranslatedPipe,
    TranslocoPipe,
  ],
  providers: [ConfigContext],
  styleUrl: './config-page.scss',
  templateUrl: './config-page.html',
})
export class ConfigPage {
  /** Id of the configuration, from the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly router = inject(Router);
  protected readonly context = inject(ConfigContext);
  protected readonly sections = CONFIG_SECTIONS;
  protected readonly projectsKey = computed(() => countKey('config.page.projects.', this.context.config()?.projectCount ?? 0));
  protected readonly busy = signal(false);

  protected readonly loaded = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getProjectConfig, { id: params }),
  });

  constructor() {
    effect(() => {
      if (this.loaded.hasValue()) this.context.config.set(this.loaded.value());
    });
  }

  protected duplicate(config: ProjectConfig): void {
    this.dialog
      .open<ConfigDialog, ConfigDialogData, ProjectConfig>(ConfigDialog, {
        data: { mode: 'duplicate', source: config },
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((copy) => {
        if (copy) void this.router.navigate(['/admin/configurations', copy.id]);
      });
  }

  protected publish(config: ProjectConfig): void {
    this.dialog
      .open<PublishDialog, ProjectConfig, ProjectConfig>(PublishDialog, {
        data: config,
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((published) => {
        if (!published) return;
        this.context.config.set(published);
        this.context.notice.set({
          ok: true,
          key: countKey('config.page.published.', published.projectCount),
          params: { number: String(published.published.number), count: String(published.projectCount) },
        });
      });
  }

  protected discard(config: ProjectConfig): void {
    const data: ConfirmDialogData = {
      titleKey: 'config.page.discard.title',
      messageKey: 'config.page.discard.message',
      confirmKey: 'config.page.discard.confirm',
      params: { name: config.name ?? '' },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        this.busy.set(true);
        try {
          const saved = await this.api.invoke(discardProjectConfigDraft, { id: config.id, 'If-Match': `"${config.version}"` });
          this.context.config.set(saved);
          this.context.notice.set({ ok: true, key: 'config.page.discard.done' });
        } catch (error) {
          this.context.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        } finally {
          this.busy.set(false);
        }
      });
  }
}
