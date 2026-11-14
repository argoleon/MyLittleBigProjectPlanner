import { Component, computed, inject, resource } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfigProject,
  ConfigProjectItem,
  EmptyState,
  ErrorState,
  ProjectConfig,
  getProjectConfig,
  listProjectConfigProjects,
} from 'ui';

import { VeiledName, isVeiled } from '../hierarchy/veiled';
import { ConfigDialog, ConfigDialogData } from './config-dialog';
import { ConfigContext } from './config-page';

/**
 * "Projects" tab of a configuration (mockup 11): the projects sharing it, by
 * name, those the user does not see veiled. A project detaches by getting
 * its own copy of the configuration.
 */
@Component({
  selector: 'app-config-projects-tab',
  imports: [
    EmptyState,
    ErrorState,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    RouterLink,
    TranslocoPipe,
    VeiledName,
  ],
  styleUrl: './config-projects-tab.scss',
  templateUrl: './config-projects-tab.html',
})
export class ConfigProjectsTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly context = inject(ConfigContext);

  protected readonly columns = ['name', 'state', 'actions'];
  private readonly id = computed(() => this.context.config()?.id);
  protected readonly projects = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(listProjectConfigProjects, { id: params }),
  });
  /** Detaching is for a configuration shared by several projects. */
  protected readonly shared = computed(() => (this.context.config()?.projectCount ?? 0) > 1);

  /** The project of a row, or null when it shows veiled (rows are untyped in the table's template). */
  protected asProject(item: ConfigProjectItem): ConfigProject | null {
    return isVeiled(item) ? null : item;
  }

  protected detach(project: ConfigProject): void {
    const source = this.context.config();
    if (!source) return;
    this.dialog
      .open<ConfigDialog, ConfigDialogData, ProjectConfig>(ConfigDialog, {
        data: { mode: 'detach', source, project },
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe(async (copy) => {
        if (!copy) return;
        this.context.notice.set({
          ok: true,
          key: 'config.projects.detached',
          params: { project: project.name, name: copy.name ?? '' },
        });
        this.projects.reload();
        // One project fewer: the chips and the tab follow.
        this.context.config.set(await this.api.invoke(getProjectConfig, { id: source.id }));
      });
  }
}
