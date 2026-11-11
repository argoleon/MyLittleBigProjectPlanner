import { Component, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatDialogModule } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  AccountSession,
  Api,
  ErrorState,
  InstantPipe,
  errorTranslationKey,
  isApiProblem,
  listMySessions,
  revokeMySession,
} from 'ui';

/** "Firefox · Linux", or what is known of it; `null` when nothing is. */
export function deviceName(session: Pick<AccountSession, 'browser' | 'os'>): string | null {
  const parts = [session.browser, session.os].filter((part): part is string => !!part);
  return parts.length ? parts.join(' · ') : null;
}

/**
 * The open sessions of the account, in every organization (L02-US08): the
 * device, the organization, sign-in and latest activity. Each one but the
 * current is signed out on its own; it is refused from its next request on.
 */
@Component({
  selector: 'app-sessions-dialog',
  imports: [ErrorState, InstantPipe, MatButtonModule, MatDialogModule, MatIconModule, MatProgressBarModule, TranslocoPipe],
  styleUrl: './sessions-dialog.scss',
  template: `
    <h2 mat-dialog-title>{{ 'account.sessions.title' | transloco }}</h2>
    <mat-dialog-content>
      <p class="intro">{{ 'account.sessions.intro' | transloco }}</p>
      @if (notice(); as notice) {
        <p class="message" [class.info]="notice.ok" [class.error]="!notice.ok" [attr.role]="notice.ok ? 'status' : 'alert'">
          {{ notice.key | transloco }}
        </p>
      }
      @if (sessions.isLoading() && !sessions.hasValue()) {
        <mat-progress-bar mode="indeterminate" [attr.aria-label]="'state.loading' | transloco" />
      } @else if (sessions.error()) {
        <ui-error-state [error]="sessions.error()" [headingLevel]="3" (retry)="sessions.reload()" />
      } @else {
        <ul class="sessions">
          @for (session of sessions.value(); track session.id) {
            <li class="session">
              <mat-icon class="icon" aria-hidden="true">{{ session.current ? 'computer' : 'devices' }}</mat-icon>
              <span class="text">
                <span class="device">
                  {{ device(session) }}
                  @if (session.current) {
                    <span class="current">{{ 'account.sessions.current' | transloco }}</span>
                  }
                </span>
                <span class="detail">
                  {{ session.tenantName ?? ('account.sessions.formerOrganization' | transloco) }} ·
                  {{ 'account.sessions.lastSeen' | transloco: { when: (session.lastSeenAt | instant: { dateStyle: 'medium', timeStyle: 'short' }) } }}
                </span>
                <span class="detail">
                  {{ 'account.sessions.signedIn' | transloco: { when: (session.createdAt | instant: { dateStyle: 'medium', timeStyle: 'short' }) } }}
                </span>
              </span>
              @if (!session.current) {
                <button
                  matButton="outlined"
                  type="button"
                  [disabled]="revoking() === session.id"
                  [attr.aria-label]="'account.sessions.revokeOf' | transloco: { device: device(session) }"
                  (click)="revoke(session)"
                >
                  {{ 'account.sessions.revoke' | transloco }}
                </button>
              }
            </li>
          }
        </ul>
      }
    </mat-dialog-content>
    <mat-dialog-actions align="end">
      <button matButton type="button" mat-dialog-close>{{ 'dialog.close' | transloco }}</button>
    </mat-dialog-actions>
  `,
})
export class SessionsDialog {
  private readonly api = inject(Api);
  private readonly transloco = inject(TranslocoService);

  protected readonly sessions = resource({ loader: () => this.api.invoke(listMySessions) });
  protected readonly revoking = signal<string | null>(null);
  protected readonly notice = signal<{ ok: boolean; key: string } | null>(null);

  protected device(session: AccountSession): string {
    return deviceName(session) ?? this.transloco.translate('account.sessions.unknownDevice');
  }

  protected async revoke(session: AccountSession): Promise<void> {
    this.revoking.set(session.id);
    this.notice.set(null);
    try {
      await this.api.invoke(revokeMySession, { id: session.id });
      this.notice.set({ ok: true, key: 'account.sessions.revoked' });
    } catch (error) {
      this.notice.set({ ok: false, key: isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown') });
    } finally {
      this.revoking.set(null);
      this.sessions.reload();
    }
  }
}
