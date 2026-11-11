import { Component, computed, effect, inject, resource, signal, viewChild } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatListModule, MatSelectionList, MatSelectionListChange } from '@angular/material/list';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  EmptyState,
  ErrorState,
  LocaleService,
  PERMISSION,
  Role,
  listRoles,
} from 'ui';

import { RoleSource, roleLabel } from './role-draft';
import { RoleEditor } from './role-editor';

/**
 * "Roles" tab of the administration (mockup 09): the tenant's node roles,
 * sets of permissions granted on hierarchy nodes (L02-US07, granted in L03).
 * Tenant admins only.
 */
@Component({
  selector: 'app-roles-tab',
  imports: [
    EmptyState,
    ErrorState,
    MatButtonModule,
    MatIconModule,
    MatListModule,
    MatProgressBarModule,
    RoleEditor,
    TranslocoPipe,
  ],
  styleUrl: './roles-tab.scss',
  template: `
    <div class="toolbar">
      <button matButton="filled" type="button" class="add" (click)="create()">
        <mat-icon aria-hidden="true">add</mat-icon>
        {{ 'admin.roles.add' | transloco }}
      </button>
    </div>
    @if (notice(); as notice) {
      <p class="message" [class.info]="notice.ok" [class.error]="!notice.ok" [attr.role]="notice.ok ? 'status' : 'alert'">
        {{ notice.key | transloco: notice.params }}
      </p>
    }
    @if (roles.isLoading() && !roles.hasValue()) {
      <mat-progress-bar mode="indeterminate" [attr.aria-label]="'state.loading' | transloco" />
    } @else if (roles.error()) {
      <ui-error-state [error]="roles.error()" [headingLevel]="2" (retry)="roles.reload()" />
    } @else {
      <div class="layout">
        @if (roles.value()?.length) {
          <mat-selection-list
            class="list"
            [multiple]="false"
            hideSingleSelectionIndicator
            [attr.aria-label]="'admin.roles.listLabel' | transloco"
            (selectionChange)="pick($event)"
          >
            @for (role of roles.value(); track role.id) {
              <mat-list-option [value]="role" [selected]="role.id === selectedId()">
                <span matListItemTitle>{{ label(role) }}</span>
                <span matListItemLine>
                  {{ 'admin.roles.permissionCount' | transloco: { count: role.permissions.length, total } }}
                  · {{ 'admin.roles.grantCount' | transloco: { count: role.grantCount } }}
                </span>
              </mat-list-option>
            }
          </mat-selection-list>
        } @else if (!selection()) {
          <ui-empty-state
            class="list"
            icon="admin_panel_settings"
            [title]="'admin.roles.empty' | transloco"
            [message]="'admin.roles.emptyHint' | transloco"
          />
        }
        @if (selection(); as source) {
          <app-role-editor
            class="detail"
            [source]="source"
            (saved)="onSaved($event)"
            (deleted)="onDeleted($event)"
            (duplicate)="open({ kind: 'new', copyOf: $event })"
          />
        }
      </div>
    }
  `,
})
export class RolesTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);
  private readonly editor = viewChild(RoleEditor);
  private readonly list = viewChild(MatSelectionList);

  protected readonly total = PERMISSION.length;
  protected readonly roles = resource({
    // Roles are few: every page at once.
    loader: async () => {
      const all: Role[] = [];
      let cursor: string | undefined;
      do {
        const page = await this.api.invoke(listRoles, { limit: 200, cursor });
        all.push(...page.items);
        cursor = page.nextCursor ?? undefined;
      } while (cursor);
      return all;
    },
  });
  /** What the detail panel shows. */
  protected readonly selection = signal<RoleSource | null>(null);
  protected readonly selectedId = computed(() => {
    const source = this.selection();
    return source?.kind === 'edit' ? source.role.id : null;
  });
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);

  constructor() {
    // Shows the first role once loaded, as long as nothing else is shown.
    effect(() => {
      if (this.selection() || this.roles.isLoading() || !this.roles.hasValue()) return;
      const [first] = this.roles.value();
      if (first) this.selection.set({ kind: 'edit', role: first });
    });
  }

  protected label(role: Role): string {
    return roleLabel(role, this.locale.language());
  }

  protected pick(event: MatSelectionListChange): void {
    const role = event.options[0]?.value as Role | undefined;
    if (role && role.id !== this.selectedId()) this.open({ kind: 'edit', role });
  }

  protected create(): void {
    this.open({ kind: 'new' });
  }

  /** Shows `source`, once unsaved changes of the current role are given up. */
  protected open(source: RoleSource): void {
    if (!this.editor()?.dirty()) {
      this.show(source);
      return;
    }
    const data: ConfirmDialogData = {
      titleKey: 'admin.roles.discardTitle',
      messageKey: 'admin.roles.discardMessage',
      confirmKey: 'admin.roles.discardConfirm',
    };
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe((confirmed) => {
        if (confirmed) this.show(source);
        else this.syncList();
      });
  }

  protected onSaved(role: Role): void {
    this.notice.set({ ok: true, key: 'admin.roles.saved', params: { label: this.label(role) } });
    this.selection.set({ kind: 'edit', role });
    this.roles.reload();
  }

  protected onDeleted(role: Role): void {
    this.notice.set({ ok: true, key: 'admin.roles.deleted', params: { label: this.label(role) } });
    this.selection.set(null);
    this.roles.reload();
  }

  /** Selects the role shown again in the list, after a pick was given up. */
  private syncList(): void {
    const id = this.selectedId();
    for (const option of this.list()?.options ?? []) {
      option.selected = (option.value as Role).id === id;
    }
  }

  private show(source: RoleSource): void {
    this.notice.set(null);
    this.selection.set(source);
  }
}
