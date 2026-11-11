import { Component, computed, inject, resource, signal } from '@angular/core';
import { toSignal } from '@angular/core/rxjs-interop';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatChipsModule } from '@angular/material/chips';
import { MatDialog } from '@angular/material/dialog';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatMenuModule } from '@angular/material/menu';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { MatTooltipModule } from '@angular/material/tooltip';
import { TranslocoPipe } from '@jsverse/transloco';
import { debounceTime } from 'rxjs';
import {
  AccountType,
  AccountTypeChip,
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  EmptyState,
  ErrorState,
  InstantPipe,
  Invitation,
  Member,
  MemberState,
  MemberStatusChip,
  errorTranslationKey,
  isApiProblem,
  listInvitations,
  listMembers,
  removeMember,
  resendInvitation,
  revokeInvitation,
} from 'ui';

import { InviteDialog } from './invite-dialog';
import { MemberDialog } from './member-dialog';

/** A filter chip of the list. */
export type MemberFilter = AccountType | 'pending';

/** A row of the table: a member, or an invitation not accepted yet. */
export type MemberRow =
  | { readonly kind: 'member'; readonly id: string; readonly member: Member }
  | { readonly kind: 'invitation'; readonly id: string; readonly invitation: Invitation };

/** What the list asks the API for, from the search field and the chips. */
export interface MemberQuery {
  readonly search: string | undefined;
  /** `undefined`: both types; `null`: no type selected, nothing to list. */
  readonly accountType: AccountType | undefined | null;
  /** Invitations only. */
  readonly pendingOnly: boolean;
}

/** The query of the list for `search` and the selected `filters`. */
export function memberQuery(search: string, filters: readonly MemberFilter[]): MemberQuery {
  const types = filters.filter((f): f is AccountType => f !== 'pending');
  return {
    search: search.trim() || undefined,
    accountType: types.length === 2 ? undefined : types.length === 1 ? types[0] : null,
    pendingOnly: filters.includes('pending'),
  };
}

/** The status chip of a row. */
export function rowState(row: MemberRow): MemberState {
  if (row.kind === 'invitation') {
    return row.invitation.status === 'expired' ? 'expired' : 'invited';
  }
  return row.member.status === 'suspended' ? 'suspended' : 'active';
}

/** Members are listed by pages of this size. */
const PAGE_SIZE = 50;

/**
 * "Members" tab of the administration (mockup 09): search, filters, invite,
 * and the table of members and pending invitations, with their type, role,
 * groups, two-step verification and status. Tenant admins only.
 */
@Component({
  selector: 'app-members-tab',
  imports: [
    AccountTypeChip,
    EmptyState,
    ErrorState,
    InstantPipe,
    MatButtonModule,
    MatChipsModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatMenuModule,
    MatProgressBarModule,
    MatTableModule,
    MatTooltipModule,
    MemberStatusChip,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  styleUrl: './members-tab.scss',
  templateUrl: './members-tab.html',
})
export class MembersTab {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  protected readonly columns = ['member', 'type', 'role', 'groups', 'mfa', 'status', 'actions'];
  protected readonly searchControl = new FormControl('', { nonNullable: true });
  protected readonly filters = signal<readonly MemberFilter[]>(['internal', 'external']);
  private readonly search = toSignal(this.searchControl.valueChanges.pipe(debounceTime(300)), { initialValue: '' });
  protected readonly query = computed(() => memberQuery(this.search(), this.filters()));

  protected readonly members = resource({
    params: () => this.query(),
    loader: async ({ params }) => {
      // A new query starts from its first page.
      this.more.set([]);
      this.moreCursor.set(undefined);
      if (params.pendingOnly || params.accountType === null) {
        return { items: [] as Member[], nextCursor: null as string | null };
      }
      return this.api.invoke(listMembers, {
        search: params.search,
        accountType: params.accountType,
        limit: PAGE_SIZE,
      });
    },
  });
  protected readonly invitations = resource({
    params: () => this.query(),
    loader: async ({ params }) => {
      if (params.accountType === null) return [] as Invitation[];
      const page = await this.api.invoke(listInvitations, {
        search: params.search,
        accountType: params.accountType,
        limit: 200,
      });
      return page.items;
    },
  });
  /** Pages loaded after the first one, and the cursor of the next. */
  private readonly more = signal<readonly Member[]>([]);
  private readonly moreCursor = signal<string | null | undefined>(undefined);
  protected readonly loadingMore = signal(false);

  protected readonly nextCursor = computed(() => {
    const more = this.moreCursor();
    if (more !== undefined && this.more().length > 0) return more;
    return this.members.hasValue() ? this.members.value().nextCursor : null;
  });

  protected readonly rows = computed<MemberRow[]>(() => {
    const invitations = this.invitations.hasValue() ? this.invitations.value() : [];
    const members = this.members.hasValue() ? [...this.members.value().items, ...this.more()] : [];
    return [
      ...invitations.map((invitation) => ({ kind: 'invitation' as const, id: invitation.id, invitation })),
      ...members.map((member) => ({ kind: 'member' as const, id: member.accountId, member })),
    ];
  });
  protected readonly loading = computed(() => this.members.isLoading() || this.invitations.isLoading());
  protected readonly error = computed(() => this.members.error() ?? this.invitations.error());

  /** Outcome of the last action, shown above the table. */
  protected readonly notice = signal<{ ok: boolean; key: string; params?: Record<string, string> } | null>(null);

  protected readonly rowState = rowState;
  protected readonly trackRow = (_: number, row: MemberRow) => row.id;

  /** The member's groups, by name. */
  protected groupNames(member: Member): string {
    return member.groups.map((group) => group.name).join(', ');
  }

  protected setFilters(filters: readonly MemberFilter[] | MemberFilter | null): void {
    this.filters.set(Array.isArray(filters) ? filters : filters ? [filters as MemberFilter] : []);
  }

  protected reload(): void {
    this.members.reload();
    this.invitations.reload();
  }

  protected async loadMore(): Promise<void> {
    const cursor = this.nextCursor();
    const query = this.query();
    if (!cursor || query.accountType === null) return;
    this.loadingMore.set(true);
    try {
      const page = await this.api.invoke(listMembers, {
        search: query.search,
        accountType: query.accountType,
        limit: PAGE_SIZE,
        cursor,
      });
      this.more.update((more) => [...more, ...page.items]);
      this.moreCursor.set(page.nextCursor ?? null);
    } catch (error) {
      this.notice.set({ ok: false, key: keyOf(error) });
    } finally {
      this.loadingMore.set(false);
    }
  }

  protected invite(): void {
    this.dialog
      .open<InviteDialog, void, Invitation>(InviteDialog, { width: 'min(520px, 100vw)', autoFocus: 'first-tabbable' })
      .afterClosed()
      .subscribe((invitation) => {
        if (!invitation) return;
        this.notice.set({ ok: true, key: 'admin.members.invited', params: { email: invitation.email } });
        this.invitations.reload();
      });
  }

  protected edit(member: Member): void {
    this.dialog
      .open<MemberDialog, Member, Member>(MemberDialog, {
        data: member,
        width: 'min(520px, 100vw)',
        autoFocus: 'first-tabbable',
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, key: 'admin.members.saved', params: { name: saved.displayName } });
        this.reload();
      });
  }

  protected remove(member: Member): void {
    const data: ConfirmDialogData = {
      titleKey: 'admin.members.removeTitle',
      messageKey: 'admin.members.removeMessage',
      confirmKey: 'admin.members.removeConfirm',
      params: { name: member.displayName },
    };
    this.confirm(data, async () => {
      await this.api.invoke(removeMember, { accountId: member.accountId, 'If-Match': `"${member.version}"` });
      this.notice.set({ ok: true, key: 'admin.members.removed', params: { name: member.displayName } });
      this.reload();
    });
  }

  protected async resend(invitation: Invitation): Promise<void> {
    try {
      await this.api.invoke(resendInvitation, { id: invitation.id });
      this.notice.set({ ok: true, key: 'admin.members.resent', params: { email: invitation.email } });
      this.invitations.reload();
    } catch (error) {
      this.notice.set({ ok: false, key: keyOf(error) });
    }
  }

  protected revoke(invitation: Invitation): void {
    const data: ConfirmDialogData = {
      titleKey: 'admin.members.revokeTitle',
      messageKey: 'admin.members.revokeMessage',
      confirmKey: 'admin.members.revokeConfirm',
      params: { email: invitation.email },
    };
    this.confirm(data, async () => {
      await this.api.invoke(revokeInvitation, { id: invitation.id, 'If-Match': `"${invitation.version}"` });
      this.notice.set({ ok: true, key: 'admin.members.revoked', params: { email: invitation.email } });
      this.invitations.reload();
    });
  }

  private confirm(data: ConfirmDialogData, action: () => Promise<void>): void {
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data, width: 'min(480px, 100vw)' })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await action();
        } catch (error) {
          this.notice.set({ ok: false, key: keyOf(error) });
        }
      });
  }
}

function keyOf(error: unknown): string {
  return isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
}
