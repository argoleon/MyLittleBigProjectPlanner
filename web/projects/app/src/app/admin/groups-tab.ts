import { Component, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  EmptyState,
  ErrorState,
  Group,
  LocalNumberPipe,
  deleteGroup,
  errorTranslationKey,
  isApiProblem,
  listGroups,
} from 'ui';

import { GroupDialog } from './group-dialog';
import { GroupMembersDialog } from './group-members-dialog';

/**
 * "Groups" tab of the administration: the tenant's groups of members, to
 * grant roles to several people at once (L02-US07). Tenant admins only.
 */
@Component({
  selector: 'app-groups-tab',
  imports: [
    EmptyState,
    ErrorState,
    LocalNumberPipe,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    TranslocoPipe,
  ],
  styleUrl: './members-tab.scss',
  templateUrl: './groups-tab.html',
})
export class GroupsTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  protected readonly columns = ['name', 'members', 'actions'];
  protected readonly groups = resource({
    // Groups are few: every page at once.
    loader: async () => {
      const all: Group[] = [];
      let cursor: string | undefined;
      do {
        const page = await this.api.invoke(listGroups, { limit: 200, cursor });
        all.push(...page.items);
        cursor = page.nextCursor ?? undefined;
      } while (cursor);
      return all;
    },
  });
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);

  protected edit(group?: Group): void {
    this.dialog
      .open<GroupDialog, Group | undefined, Group>(GroupDialog, {
        data: group,
        width: 'min(520px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, key: 'admin.groups.saved', params: { name: saved.name } });
        this.groups.reload();
      });
  }

  protected openMembers(group: Group): void {
    this.dialog
      .open<GroupMembersDialog, Group>(GroupMembersDialog, {
        data: group,
        width: 'min(600px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe(() => this.groups.reload());
  }

  protected remove(group: Group): void {
    const data: ConfirmDialogData = {
      titleKey: 'admin.groups.deleteTitle',
      messageKey: 'admin.groups.deleteMessage',
      confirmKey: 'admin.groups.deleteConfirm',
      params: { name: group.name },
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteGroup, { id: group.id, 'If-Match': `"${group.version}"` });
          this.notice.set({ ok: true, key: 'admin.groups.deleted', params: { name: group.name } });
          this.groups.reload();
        } catch (error) {
          this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }
}
