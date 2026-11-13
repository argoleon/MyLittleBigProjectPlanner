import { Component, ElementRef, inject, input, resource, signal, viewChild } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  AccessRole,
  Api,
  ErrorState,
  LocaleService,
  Node,
  NodeAccess,
  RoleChip,
  errorTranslationKey,
  isApiProblem,
  listNodeAccess,
  revokeNodeRole,
} from 'ui';

import { roleLabel } from '../admin/role-draft';
import { GrantDialog, GrantDialogData } from './grant-dialog';
import { HOLDER_ICONS, Label, originLabel } from './node-access';

/**
 * "Effective access" card of a node (mockup 14): every member or group with
 * a role there, and whether the role is granted on the node or inherited
 * from which ancestor. Roles are granted from its "Add" dialog; a direct
 * one is revoked from its row.
 */
@Component({
  selector: 'app-node-access-card',
  imports: [ErrorState, MatButtonModule, MatIconModule, MatProgressBarModule, RoleChip, TranslocoPipe],
  templateUrl: './node-access-card.html',
  styleUrl: './node-access-card.scss',
})
export class NodeAccessCard {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);

  /** The node; a new value (its parents changed) reloads the access. */
  readonly node = input.required<Node>();

  protected readonly icons = HOLDER_ICONS;
  protected readonly origin = (entry: NodeAccess): Label => originLabel(entry, this.node().type);
  /** The role's label in the language of the interface. */
  protected readonly roleName = (role: AccessRole): string => roleLabel(role, this.locale.language());
  protected readonly access = resource({
    params: () => this.node(),
    loader: ({ params }) => this.api.invoke(listNodeAccess, { id: params.id }),
  });
  protected readonly busy = signal(false);
  protected readonly notice = signal<(Label & { ok: boolean }) | null>(null);

  private readonly addButton = viewChild('addButton', { read: ElementRef<HTMLButtonElement> });

  protected add(): void {
    const node = this.node();
    const data: GrantDialogData = { node: { id: node.id, name: node.name } };
    this.dialog
      .open<GrantDialog, GrantDialogData, NodeAccess>(GrantDialog, { data, width: 'min(520px, 100vw)' })
      .afterClosed()
      .subscribe((granted) => {
        if (!granted) return;
        this.notice.set({
          ok: true,
          key: 'hierarchy.access.granted',
          params: { role: this.roleName(granted.role), name: granted.holder.name },
        });
        this.access.reload();
      });
  }

  protected async revoke(entry: NodeAccess): Promise<void> {
    this.busy.set(true);
    this.notice.set(null);
    try {
      await this.api.invoke(revokeNodeRole, { id: this.node().id, grantId: entry.grantId });
      this.notice.set({
        ok: true,
        key: 'hierarchy.access.revoked',
        params: { role: this.roleName(entry.role), name: entry.holder.name },
      });
      // The row and its button are gone.
      this.addButton()?.nativeElement.focus();
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.busy.set(false);
      this.access.reload();
    }
  }
}
