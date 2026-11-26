/**
 * Tests of the notifications page (mockup 16, L05-US07), the API mocked at
 * the HTTP level and the real translations: notifications of two
 * organizations grouped by period, filtered by kind and by organization;
 * one opened (marked read, then its ticket, or its organization); all
 * marked read; preferences changed per event and channel, the digest; the
 * states; accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { Router, provideRouter } from '@angular/router';
import { AuthService, MemberNotification, NotificationPreferences, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { NotificationCenter } from './notification-center';
import { NotificationsPage, periodOf } from './notifications-page';

const ACME = { id: 't1', slug: 'acme', name: 'Acme Industrie' };
const SERVICES = { id: 't2', slug: 'services', name: 'Acme Services' };
const MEMBERSHIPS = [
  { tenantId: ACME.id, slug: ACME.slug, name: ACME.name, accountType: 'internal', tenantRole: 'member', current: true },
  { tenantId: SERVICES.id, slug: SERVICES.slug, name: SERVICES.name, accountType: 'internal', tenantRole: 'member', current: false },
];

const now = new Date();
const hoursAgo = (hours: number) => new Date(now.getTime() - hours * 3_600_000).toISOString();

const notification = (id: string, rest: Partial<MemberNotification>): MemberNotification => ({
  id,
  tenant: ACME,
  event: 'mentioned',
  issueId: `i-${id}`,
  issueKey: 'PLAT-151',
  issueSummary: 'MFA policy',
  actorName: 'Inès Moreau',
  status: null,
  excerpt: null,
  export: null,
  bulk: null,
  conflict: null,
  read: false,
  createdAt: hoursAgo(0.1),
  ...rest,
});

/** An export of the member's ready: no ticket. */
const EXPORT_READY = notification('n4', {
  event: 'export_ready',
  issueId: null,
  issueKey: null,
  issueSummary: null,
  actorName: null,
  export: { id: 'e1', fileName: 'search-2026-11-18.csv', rows: 20001 },
});

/** A bulk action of the member's done in the background: no ticket. */
const BULK_DONE = notification('n5', {
  event: 'bulk_done',
  issueId: null,
  issueKey: null,
  issueSummary: null,
  actorName: null,
  bulk: { id: 'b1', kind: 'transition', total: 250, changed: 240, unchanged: 4, refused: 6 },
});

/** Hours of a ticket an absence leaves no room for (L08-US04). */
const CONFLICT = notification('n6', {
  event: 'absence_conflict',
  issueKey: 'PROJ-42',
  issueSummary: 'Account migration scripts',
  actorName: null,
  conflict: { resourceName: 'Nathan Roux', minutes: 420 },
});

const ITEMS: MemberNotification[] = [
  notification('n1', { excerpt: '@Lucas can you check?' }),
  notification('n2', { tenant: SERVICES, event: 'assigned', issueKey: 'SUP-1042', issueSummary: 'Mail access' }),
  notification('n3', { event: 'status_changed', status: 'In progress', read: true, createdAt: hoursAgo(24 * 30) }),
];

const PREFERENCES: NotificationPreferences = {
  events: [
    { event: 'created', inApp: true, email: true },
    { event: 'assigned', inApp: true, email: true },
    { event: 'status_changed', inApp: true, email: false },
    { event: 'commented', inApp: true, email: false },
    { event: 'mentioned', inApp: true, email: true },
    { event: 'absence_conflict', inApp: true, email: true },
    { event: 'export_ready', inApp: true, email: false },
    { event: 'bulk_done', inApp: true, email: true },
  ],
  dailyDigest: false,
};

describe('periodOf', () => {
  it('groups by today, the last seven days and before', () => {
    const today = new Date(2026, 10, 15, 14, 0);
    expect(periodOf(new Date(2026, 10, 15, 8, 0).toISOString(), today)).toBe('today');
    expect(periodOf(new Date(2026, 10, 14, 23, 0).toISOString(), today)).toBe('week');
    expect(periodOf(new Date(2026, 10, 9, 9, 0).toISOString(), today)).toBe('week');
    expect(periodOf(new Date(2026, 10, 8, 9, 0).toISOString(), today)).toBe('earlier');
  });
});

describe('the notifications page', () => {
  async function open(items: MemberNotification[] = ITEMS) {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [provideRouter([]), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const http = TestBed.inject(HttpTestingController);
    const fixture = TestBed.createComponent(NotificationsPage);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve));
        fixture.detectChanges();
      }
    };
    const reply = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams, r.method));
        if (!request) {
          fixture.detectChanges();
          await new Promise((resolve) => setTimeout(resolve, 10));
        }
      }
      if (!request) throw new Error('no matching request');
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const isList = (url: string, method: string) => url.startsWith('/api/v1/me/notifications?') && method === 'GET';
    await reply((url) => url === '/api/v1/me/memberships', MEMBERSHIPS);
    await reply((url) => url === '/api/v1/me/notification-preferences', PREFERENCES);
    await reply(isList, { items, nextCursor: null });
    TestBed.inject(NotificationCenter).update(2);
    await settle();
    const root = fixture.nativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => {
      const copy = element?.cloneNode(true) as Element | undefined;
      copy?.querySelectorAll('mat-icon').forEach((icon) => icon.remove());
      return copy?.textContent?.replace(/\s+/g, ' ').trim();
    };
    const texts = (selector: string) => Array.from(root.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    // A chip is selected through its action, not its host.
    const chip = (label: string) =>
      Array.from(root.querySelectorAll('mat-chip-option'))
        .find((c) => text(c) === label)
        ?.querySelector('.mat-mdc-chip-action');
    return { http, fixture, root, reply, settle, text, texts, click, chip, isList };
  }

  it('lists the notifications of every organization by period, the unread ones marked', async () => {
    const { root, text, texts } = await open();
    expect(text(root.querySelector('.subtitle'))).toBe('2 unread · all organizations');
    expect(texts('.period')).toEqual(['Today', 'Earlier']);
    expect(texts('.item .what')).toEqual([
      'Inès Moreau mentioned you on PLAT-151 (unread)',
      'Inès Moreau assigned SUP-1042 to you (unread)',
      'Inès Moreau moved PLAT-151 to In progress',
    ]);
    expect(texts('.item .detail')).toEqual(['« @Lucas can you check? »', 'Mail access', 'MFA policy']);
    expect(root.querySelectorAll('.item.unread')).toHaveLength(2);
    expect(texts('.item .elsewhere')).toEqual(['opens in Acme Services']);
    expect(await axeViolations(root)).toEqual([]);
  });

  it('filters by kind, and by organization through the API', async () => {
    const { texts, click, chip, reply, isList } = await open();
    await click(chip('Assignments'));
    expect(texts('.item .what')).toEqual(['Inès Moreau assigned SUP-1042 to you (unread)']);
    await click(chip('All'));
    await click(chip('Acme Services'));
    const filtered = await reply(isList, { items: [ITEMS[1]], nextCursor: null });
    expect(filtered.request.params.get('tenantId')).toBe('t2');
    expect(texts('.item .what')).toHaveLength(1);
  });

  it('opens a notification of this organization: marked read, then its ticket', async () => {
    const { root, click, reply, settle } = await open();
    const navigate = vi.spyOn(TestBed.inject(Router), 'navigate').mockResolvedValue(true);
    await click(root.querySelector('.item'));
    const marked = await reply((url, method) => url === '/api/v1/me/notifications/read' && method === 'POST', {
      total: 1,
      byTenant: [{ tenantId: 't2', unread: 1 }],
    });
    expect(marked.request.body).toEqual({ ids: ['n1'] });
    await settle();
    expect(navigate).toHaveBeenCalledWith(['/issues', 'PLAT-151']);
    expect(TestBed.inject(NotificationCenter).unread()).toBe(1);
    expect(root.querySelectorAll('.item.unread')).toHaveLength(1);
  });

  it('shows an export ready, and opens it', async () => {
    const { root, click, reply, settle, texts } = await open([EXPORT_READY]);
    expect(texts('.item .what')).toEqual(['Your export search-2026-11-18.csv is ready (unread)']);
    expect(texts('.item .detail')).toEqual(['20001 rows']);
    expect(root.querySelector('.item mat-icon')?.textContent).toBe('download');
    const navigate = vi.spyOn(TestBed.inject(Router), 'navigate').mockResolvedValue(true);
    await click(root.querySelector('.item'));
    await reply((url, method) => url === '/api/v1/me/notifications/read' && method === 'POST', { total: 0, byTenant: [] });
    await settle();
    expect(navigate).toHaveBeenCalledWith(['/exports', 'e1']);
    expect(root.querySelector<HTMLInputElement>('input[aria-label="Export ready by email"]')?.checked).toBe(false);
  });

  it('shows the hours to reschedule of an absence conflict, and opens the ticket', async () => {
    const { root, click, reply, settle, texts } = await open([CONFLICT]);
    expect(texts('.item .what')).toEqual(['7 h to reschedule on PROJ-42 (absence of Nathan Roux) (unread)']);
    expect(root.querySelector('.item mat-icon')?.textContent).toBe('event_busy');
    expect(root.querySelector<HTMLInputElement>('input[aria-label="Absence conflict by email"]')?.checked).toBe(true);
    const navigate = vi.spyOn(TestBed.inject(Router), 'navigate').mockResolvedValue(true);
    await click(root.querySelector('.item'));
    await reply((url, method) => url === '/api/v1/me/notifications/read' && method === 'POST', { total: 0, byTenant: [] });
    await settle();
    expect(navigate).toHaveBeenCalledWith(['/issues', 'PROJ-42']);
  });

  it('shows a bulk action done, and opens its report', async () => {
    const { root, click, reply, settle, texts } = await open([BULK_DONE]);
    expect(texts('.item .what')).toEqual(['Your bulk status change is done (unread)']);
    expect(texts('.item .detail')).toEqual(['240 changed, 4 already so, 6 left unchanged']);
    expect(root.querySelector('.item mat-icon')?.textContent).toBe('library_add_check');
    const navigate = vi.spyOn(TestBed.inject(Router), 'navigate').mockResolvedValue(true);
    await click(root.querySelector('.item'));
    await reply((url, method) => url === '/api/v1/me/notifications/read' && method === 'POST', { total: 0, byTenant: [] });
    await settle();
    expect(navigate).toHaveBeenCalledWith(['/bulk-actions', 'b1']);
    expect(root.querySelector<HTMLInputElement>('input[aria-label="Bulk action done by email"]')?.checked).toBe(true);
  });

  it("opens another organization's notification in that organization", async () => {
    const { root, click, reply, settle } = await open();
    const switchTenant = vi.spyOn(TestBed.inject(AuthService), 'switchTenant').mockResolvedValue();
    await click(root.querySelectorAll('.item')[1]);
    await reply((url) => url === '/api/v1/me/notifications/read', { total: 1, byTenant: [] });
    await settle();
    expect(switchTenant).toHaveBeenCalledWith('t2');
  });

  it('marks all read, and says when it cannot', async () => {
    const { root, click, reply } = await open();
    await click(root.querySelector('button.mark-all'));
    const all = await reply((url) => url === '/api/v1/me/notifications/read', { total: 0, byTenant: [] });
    expect(all.request.body).toEqual({ tenantId: undefined });
    expect(root.querySelectorAll('.item.unread')).toHaveLength(0);
    expect((root.querySelector('button.mark-all') as HTMLButtonElement).disabled).toBe(true);
  });

  it('changes a preference and the digest, for every organization', async () => {
    const { root, click, reply, text } = await open();
    expect(text(root.querySelector('.preferences .hint'))).toBe('They apply in all your organizations.');
    const statusEmail = root.querySelector<HTMLInputElement>('input[aria-label="Status change by email"]')!;
    expect(statusEmail.checked).toBe(false);
    await click(statusEmail);
    const saved = await reply((url, method) => url === '/api/v1/me/notification-preferences' && method === 'PUT', {
      ...PREFERENCES,
      events: PREFERENCES.events.map((c) => (c.event === 'status_changed' ? { ...c, email: true } : c)),
    });
    expect((saved.request.body as NotificationPreferences).events[2]).toEqual({ event: 'status_changed', inApp: true, email: true });
    await click(root.querySelector('.digest button[role="switch"]'));
    const digest = await reply((url, method) => url.endsWith('/notification-preferences') && method === 'PUT', { ...PREFERENCES, dailyDigest: true });
    expect((digest.request.body as NotificationPreferences).dailyDigest).toBe(true);
    expect(await axeViolations(root.querySelector('.preferences')!)).toEqual([]);
  });

  it('says when there is nothing, and when the list cannot be read', async () => {
    const { root, chip, click, reply, isList, text } = await open();
    await click(chip('New tickets'));
    expect(text(root.querySelector('.empty'))).toBe('No notification.');
    await click(chip('All'));
    await click(chip('Acme Services'));
    await reply(isList, { type: 'about:blank', title: 'Unavailable', status: 503, code: 'unavailable' }, 503);
    expect(root.querySelector('ui-error-state')).not.toBeNull();
  });
});
