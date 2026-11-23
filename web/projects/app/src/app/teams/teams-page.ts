import { Component, computed, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatTableModule } from '@angular/material/table';
import { RouterLink } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ConfirmDialog,
  ConfirmDialogData,
  EmptyState,
  ErrorState,
  Team,
  deleteTeam,
  errorTranslationKey,
  isApiProblem,
  listTeams,
} from 'ui';

import { TeamDialog, TeamDialogData } from './team-dialog';

/** Outcome of the last action, shown above the table. */
interface Notice {
  readonly ok: boolean;
  readonly messageKey: string;
  readonly params?: Record<string, string>;
}

/**
 * "Teams" tab of Workload (specification §8, L07-US02): the teams of the
 * tenant, each opening its page. Every internal member reads them; tenant
 * admins and resource managers create, rename and delete them.
 */
@Component({
  selector: 'app-teams-page',
  imports: [EmptyState, ErrorState, MatButtonModule, MatIconModule, MatProgressBarModule, MatTableModule, RouterLink, TranslocoPipe],
  templateUrl: './teams-page.html',
  styleUrl: './teams-page.scss',
})
export class TeamsPage {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);

  protected readonly list = resource({ loader: () => this.api.invoke(listTeams) });
  protected readonly teams = computed<readonly Team[]>(() => (this.list.hasValue() ? this.list.value().items : []));
  protected readonly canManage = computed(() => this.list.hasValue() && this.list.value().canManage);
  protected readonly columns = computed(() => (this.canManage() ? ['name', 'members', 'actions'] : ['name', 'members']));
  protected readonly notice = signal<Notice | null>(null);
  protected readonly trackTeam = (_: number, team: Team) => team.id;

  /** `id` of the actions of `team`, where the focus goes back. */
  protected actionsId(team: Team, action: 'edit' | 'delete'): string {
    return `team-${action}-${team.id}`;
  }

  protected create(): void {
    this.open({}, undefined, 'teams.created');
  }

  protected edit(team: Team): void {
    this.open({ team }, `#${this.actionsId(team, 'edit')}`, 'teams.saved');
  }

  protected remove(team: Team): void {
    const name = team.name ?? '';
    this.dialog
      .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
        data: {
          titleKey: 'teams.deleteTitle',
          messageKey: 'teams.deleteMessage',
          confirmKey: 'teams.deleteConfirm',
          params: { name },
        },
        width: 'min(480px, 100vw)',
        restoreFocus: `#${this.actionsId(team, 'delete')}`,
      })
      .afterClosed()
      .subscribe(async (confirmed) => {
        if (!confirmed) return;
        try {
          await this.api.invoke(deleteTeam, { id: team.id, 'If-Match': `"${team.version}"` });
          this.notice.set({ ok: true, messageKey: 'teams.deleted', params: { name } });
          this.list.reload();
        } catch (error) {
          this.notice.set({ ok: false, messageKey: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
        }
      });
  }

  private open(data: TeamDialogData, restoreFocus: string | undefined, messageKey: string): void {
    this.dialog
      .open<TeamDialog, TeamDialogData, Team>(TeamDialog, {
        data,
        width: 'min(560px, 100vw)',
        autoFocus: 'first-tabbable',
        restoreFocus: restoreFocus ?? true,
      })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set({ ok: true, messageKey, params: { name: saved.name ?? '' } });
        this.list.reload();
      });
  }
}
