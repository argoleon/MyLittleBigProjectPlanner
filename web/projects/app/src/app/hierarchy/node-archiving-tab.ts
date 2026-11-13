import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { TranslocoPipe } from '@jsverse/transloco';
import { firstValueFrom } from 'rxjs';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  Node,
  errorTranslationKey,
  isApiProblem,
  listMyNodePermissions,
  setNodeArchiving,
} from 'ui';

import { NodeContext } from './node-settings-page';

/**
 * "Archiving" tab of a project's settings (mockup 14): archives a finished
 * project, or restores it, after a confirmation that says what follows.
 * Only those who administer the project do it; the others read its state.
 */
@Component({
  selector: 'app-node-archiving-tab',
  imports: [MatButtonModule, MatIconModule, TranslocoPipe],
  templateUrl: './node-archiving-tab.html',
  styleUrl: './node-archiving-tab.scss',
})
export class NodeArchivingTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly context = inject(NodeContext);

  protected readonly node = this.context.node;
  private readonly id = computed(() => this.node()?.id);
  protected readonly permissions = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(listMyNodePermissions, { id: params }),
  });
  /** Whether the member administers the project: archiving it is theirs. */
  protected readonly administers = computed(
    () => this.permissions.hasValue() && this.permissions.value().includes('administer_node'),
  );
  protected readonly saving = signal(false);
  protected readonly notice = signal<{ ok: boolean; key: string } | null>(null);

  protected async toggle(node: Node): Promise<void> {
    const archive = !node.archived;
    const data: ConfirmDialogData = {
      titleKey: archive ? 'hierarchy.archiving.confirmArchive.title' : 'hierarchy.archiving.confirmRestore.title',
      messageKey: archive ? 'hierarchy.archiving.confirmArchive.message' : 'hierarchy.archiving.confirmRestore.message',
      confirmKey: archive ? 'hierarchy.archiving.archive' : 'hierarchy.archiving.restore',
      params: { name: node.name },
    };
    const confirmed = await firstValueFrom(
      this.dialog.open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' }).afterClosed(),
    );
    if (!confirmed) return;
    this.saving.set(true);
    this.notice.set(null);
    try {
      const saved = await this.api.invoke(setNodeArchiving, {
        id: node.id,
        'If-Match': `"${node.version}"`,
        body: { archived: archive },
      });
      this.context.node.set(saved);
      this.notice.set({ ok: true, key: saved.archived ? 'hierarchy.archiving.archived' : 'hierarchy.archiving.restored' });
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.saving.set(false);
    }
  }
}
