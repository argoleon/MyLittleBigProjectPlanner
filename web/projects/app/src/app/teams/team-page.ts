import { Component, computed, effect, inject, input, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  Avatar,
  EmptyState,
  ErrorState,
  LoadingState,
  Resource,
  ResourceKindChip,
  ResourceStateChip,
  Team,
  errorTranslationKey,
  getTeam,
  isApiProblem,
  removeTeamMember,
  resourceHolding,
} from 'ui';

import { managesResources } from '../resources/resource-page';
import { ResourcesStore } from '../resources/resources-store';
import { AddMemberData, AddMemberDialog } from './add-member-dialog';
import { TeamDialog, TeamDialogData } from './team-dialog';

/** Outcome of the last action, shown above the table. */
interface Notice {
  readonly ok: boolean;
  readonly messageKey: string;
  readonly params?: Record<string, string>;
}

/**
 * The page of a team (L07-US02): its name, description and resources,
 * active or not. Tenant admins and resource managers add and remove
 * resources; removing one leaves its assignments untouched.
 */
@Component({
  selector: 'app-team-page',
  imports: [
    Avatar,
    EmptyState,
    ErrorState,
    LoadingState,
    MatButtonModule,
    MatIconModule,
    MatProgressBarModule,
    MatTableModule,
    ResourceKindChip,
    ResourceStateChip,
    RouterLink,
    TranslocoPipe,
  ],
  providers: [ResourcesStore],
  templateUrl: './team-page.html',
  styleUrl: './team-page.scss',
})
export class TeamPage {
  /** `:id` of the route. */
  readonly id = input.required<string>();

  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly dialog = inject(MatDialog);
  protected readonly members = inject(ResourcesStore);

  protected readonly current = resource({
    params: () => this.id(),
    loader: ({ params }) => this.api.invoke(getTeam, { id: params }),
  });
  /** Saved by the dialog since the page was loaded. */
  private readonly saved = signal<Team | null>(null);
  protected readonly team = computed<Team | null>(() => {
    const saved = this.saved();
    if (saved?.id === this.id()) return saved;
    return this.current.hasValue() ? this.current.value() : null;
  });
  protected readonly canManage = computed(() => managesResources(this.auth.session()?.account.tenantRole));
  protected readonly columns = computed(() => (this.canManage() ? ['name', 'holding', 'status', 'actions'] : ['name', 'holding', 'status']));
  protected readonly notice = signal<Notice | null>(null);
  protected readonly holding = resourceHolding;
  protected readonly trackResource = (_: number, row: Resource) => row.id;

  constructor() {
    // Every member of the team, active or not, once its id is known.
    this.members.states.set(['active', 'inactive']);
    this.members.teamOnly.set(true);
    effect(() => this.members.team.set(this.id()));
  }

  /** `id` of the remove button of `row`. */
  protected removeId(row: Resource): string {
    return `team-remove-${row.id}`;
  }

  protected edit(team: Team): void {
    this.dialog
      .open<TeamDialog, TeamDialogData, Team>(TeamDialog, {
        data: { team },
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: '#team-edit',
      })
      .afterClosed()
      .subscribe((saved) => {
        if (saved) this.saved.set(saved);
      });
  }

  protected add(team: Team): void {
    this.dialog
      .open<AddMemberDialog, AddMemberData, Resource>(AddMemberDialog, {
        data: { team },
        width: 'min(520px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: '#team-add',
      })
      .afterClosed()
      .subscribe((added) => {
        if (!added) return;
        this.notice.set({ ok: true, messageKey: 'teams.page.added', params: { name: added.displayName } });
        this.refresh();
      });
  }

  protected async remove(team: Team, row: Resource): Promise<void> {
    try {
      await this.api.invoke(removeTeamMember, { id: team.id, resourceId: row.id });
      this.notice.set({ ok: true, messageKey: 'teams.page.removed', params: { name: row.displayName } });
      this.refresh();
    } catch (error) {
      this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    }
  }

  /** The member count and the members changed. */
  private refresh(): void {
    this.saved.set(null);
    this.current.reload();
    this.members.reload();
  }
}
