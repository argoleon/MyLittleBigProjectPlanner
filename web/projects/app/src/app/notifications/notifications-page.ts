import { Component, computed, effect, inject, resource, signal } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatCheckboxModule } from '@angular/material/checkbox';
import { MatChipsModule } from '@angular/material/chips';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { Router } from '@angular/router';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  AuthService,
  ErrorState,
  InstantPipe,
  MemberNotification,
  MemberNotificationEvent,
  NotificationPreferences,
  errorTranslationKey,
  getMyNotificationPreferences,
  isApiProblem,
  listMyMemberships,
  listMyNotifications,
  markMyNotificationsRead,
  setMyNotificationPreferences,
} from 'ui';

import { NotificationCenter } from './notification-center';

/** Notifications read at a time. */
export const NOTIFICATION_PAGE = 30;

/** The kinds of notification a member filters on. */
export const KINDS: readonly MemberNotificationEvent[] = ['mentioned', 'assigned', 'status_changed', 'commented', 'created', 'export_ready', 'bulk_done'];

const ICONS: Record<MemberNotificationEvent, string> = {
  mentioned: 'alternate_email',
  assigned: 'assignment_ind',
  status_changed: 'swap_horiz',
  commented: 'chat',
  created: 'add_task',
  export_ready: 'download',
  bulk_done: 'library_add_check',
};

/** When a notification came, to group them: today, the last seven days, before. */
export function periodOf(createdAt: string, now: Date): 'today' | 'week' | 'earlier' {
  const at = new Date(createdAt);
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  if (at >= startOfToday) return 'today';
  const weekAgo = new Date(startOfToday);
  weekAgo.setDate(weekAgo.getDate() - 6);
  return at >= weekAgo ? 'week' : 'earlier';
}

/**
 * The notifications of the member (mockup 16, L05-US07): those of all
 * their organizations, newest first, grouped by period, filtered by kind
 * and by organization; opened (which marks them read) or all marked read;
 * and their preferences, per event in the application and by email, and
 * the daily digest, for every organization.
 */
@Component({
  selector: 'app-notifications-page',
  imports: [
    ErrorState,
    InstantPipe,
    MatButtonModule,
    MatCheckboxModule,
    MatChipsModule,
    MatIconModule,
    MatProgressBarModule,
    MatSlideToggleModule,
    TranslocoPipe,
  ],
  templateUrl: './notifications-page.html',
  styleUrl: './notifications-page.scss',
})
export class NotificationsPage {
  private readonly api = inject(Api);
  private readonly auth = inject(AuthService);
  private readonly router = inject(Router);
  private readonly center = inject(NotificationCenter);

  protected readonly kinds = KINDS;
  protected readonly icons = ICONS;
  protected readonly memberships = resource({ loader: () => this.api.invoke(listMyMemberships) });
  protected readonly currentTenant = computed(() =>
    this.memberships.hasValue() ? (this.memberships.value().find((m) => m.current)?.tenantId ?? null) : null,
  );
  protected readonly tenants = computed(() => (this.memberships.hasValue() ? this.memberships.value() : []));

  /** The organization shown (none: all) and the kind (none: all). */
  protected readonly tenant = signal<string | null>(null);
  protected readonly kind = signal<MemberNotificationEvent | null>(null);

  protected readonly items = signal<MemberNotification[] | null>(null);
  protected readonly cursor = signal<string | null>(null);
  protected readonly loading = signal(false);
  protected readonly error = signal<unknown>(null);
  protected readonly errorKey = signal<string | null>(null);

  protected readonly shown = computed(() => {
    const kind = this.kind();
    return (this.items() ?? []).filter((n) => !kind || n.event === kind);
  });
  protected readonly groups = computed(() => {
    const now = new Date();
    const groups: { period: 'today' | 'week' | 'earlier'; items: MemberNotification[] }[] = [];
    for (const notification of this.shown()) {
      const period = periodOf(notification.createdAt, now);
      const group = groups.find((g) => g.period === period);
      if (group) group.items.push(notification);
      else groups.push({ period, items: [notification] });
    }
    return groups;
  });
  protected readonly unread = this.center.unread;

  protected readonly preferences = resource({ loader: () => this.api.invoke(getMyNotificationPreferences) });
  protected readonly preferencesErrorKey = signal<string | null>(null);

  constructor() {
    // Read again whenever the organization filter changes.
    effect(() => {
      this.tenant();
      void this.load(true);
    });
  }

  async load(fresh: boolean): Promise<void> {
    this.loading.set(true);
    this.error.set(null);
    try {
      const page = await this.api.invoke(listMyNotifications, {
        tenantId: this.tenant() ?? undefined,
        limit: NOTIFICATION_PAGE,
        cursor: fresh ? undefined : (this.cursor() ?? undefined),
      });
      this.items.update((items) => (fresh ? page.items : [...(items ?? []), ...page.items]));
      this.cursor.set(page.nextCursor);
    } catch (error) {
      this.error.set(error);
    } finally {
      this.loading.set(false);
    }
  }

  private failure(error: unknown): string {
    return isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown');
  }

  /** Opens the ticket (or the export) of `notification`, marking it read; in its organization if another. */
  protected async open(notification: MemberNotification): Promise<void> {
    this.errorKey.set(null);
    try {
      if (!notification.read) {
        const left = await this.api.invoke(markMyNotificationsRead, { body: { ids: [notification.id] } });
        this.center.update(left.total);
        this.items.update((items) => (items ?? []).map((n) => (n.id === notification.id ? { ...n, read: true } : n)));
      }
      if (notification.tenant.id === this.currentTenant()) {
        await this.router.navigate(
          notification.export
            ? ['/exports', notification.export.id]
            : notification.bulk
              ? ['/bulk-actions', notification.bulk.id]
              : ['/issues', notification.issueKey],
        );
      } else {
        await this.auth.switchTenant(notification.tenant.id);
      }
    } catch (error) {
      this.errorKey.set(this.failure(error));
    }
  }

  protected async markAll(): Promise<void> {
    this.errorKey.set(null);
    try {
      const left = await this.api.invoke(markMyNotificationsRead, { body: { tenantId: this.tenant() ?? undefined } });
      this.center.update(left.total);
      this.items.update((items) => (items ?? []).map((n) => ({ ...n, read: true })));
    } catch (error) {
      this.errorKey.set(this.failure(error));
    }
  }

  protected inOtherTenant(notification: MemberNotification): boolean {
    return notification.tenant.id !== this.currentTenant();
  }

  /** Saves `change` to the preferences. */
  protected async choose(change: (preferences: NotificationPreferences) => NotificationPreferences): Promise<void> {
    if (!this.preferences.hasValue()) return;
    this.preferencesErrorKey.set(null);
    const wanted = change(this.preferences.value());
    this.preferences.set(wanted);
    try {
      this.preferences.set(await this.api.invoke(setMyNotificationPreferences, { body: wanted }));
    } catch (error) {
      this.preferencesErrorKey.set(this.failure(error));
      this.preferences.reload();
    }
  }

  protected toggle(event: MemberNotificationEvent, channel: 'inApp' | 'email', on: boolean): void {
    void this.choose((preferences) => ({
      ...preferences,
      events: preferences.events.map((choice) => (choice.event === event ? { ...choice, [channel]: on } : choice)),
    }));
  }

  protected digest(on: boolean): void {
    void this.choose((preferences) => ({ ...preferences, dailyDigest: on }));
  }
}
