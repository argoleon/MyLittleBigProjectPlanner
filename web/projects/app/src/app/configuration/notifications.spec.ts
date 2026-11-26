/**
 * Tests of the Notifications tab of a configuration (L04-US07): the
 * recipients each event offers and shows, then the tab in the
 * configuration page with the API mocked at the HTTP level and the real
 * translations: the rules by event, the edit dialog with its people,
 * roles, groups and members, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import {
  AuthService,
  NotificationRule,
  NotificationRuleList,
  ProjectConfig,
  TransitionRuleReferences,
  provideApi,
  provideMaterialDefaults,
} from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ConfigPage } from './config-page';
import { kindsFor, recipientsOf, withRef, withoutRef } from './notifications';
import { NotificationsTab } from './notifications-tab';

const CONFIG: ProjectConfig = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7a02',
  name: 'Software development',
  nameTranslations: { fr: 'Développement logiciel', en: 'Software development' },
  isDefault: false,
  projectCount: 6,
  published: { number: 2, publishedAt: '2026-11-12T09:00:00Z' },
  draft: null,
  version: 5,
};

const rule = (event: NotificationRule['event'], rest: Partial<NotificationRule> = {}): NotificationRule => ({
  event,
  recipientKinds: [],
  roleIds: [],
  groupIds: [],
  accountIds: [],
  version: 1,
  ...rest,
});

const REFERENCES: TransitionRuleReferences = {
  roles: [{ id: 'r1', label: 'Project manager', labelTranslations: { fr: 'Chef de projet', en: 'Project manager' } }],
  groups: [{ id: 'g1', name: 'Quality' }],
  members: [{ id: 'm1', name: 'Bob Martin' }],
};

const RULES: NotificationRuleList = {
  stage: 'published',
  items: [
    rule('created', { recipientKinds: ['reporter'], roleIds: ['r1', 'r-gone'] }),
    rule('assigned', { recipientKinds: ['assignee'] }),
    rule('status_changed'),
    rule('commented', { recipientKinds: ['watchers', 'mentioned'], groupIds: ['g1'], accountIds: ['m1'] }),
    rule('mentioned', { recipientKinds: ['mentioned'] }),
    rule('absence_conflict', { recipientKinds: ['assignee'], roleIds: ['r1'] }),
  ],
  references: REFERENCES,
};

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'configuration_admin' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

describe('the recipients of an event', () => {
  it('offer the people mentioned only where mentions happen', () => {
    expect(kindsFor('created')).toEqual(['reporter', 'assignee', 'watchers']);
    expect(kindsFor('commented')).toEqual(['reporter', 'assignee', 'watchers', 'mentioned']);
    expect(kindsFor('mentioned')).toContain('mentioned');
    expect(kindsFor('absence_conflict')).toEqual(['reporter', 'assignee', 'watchers']);
  });

  it('show the people of the ticket, then roles, groups and members by name', () => {
    const recipients = recipientsOf(RULES.items[3], REFERENCES, (labels) => labels['en'] ?? '');
    expect(recipients.map((r) => r.key ?? r.name)).toEqual([
      'config.notifications.kinds.watchers',
      'config.notifications.kinds.mentioned',
      'Quality',
      'Bob Martin',
    ]);
    // A role deleted since: no name.
    expect(recipientsOf(RULES.items[0], REFERENCES, (labels) => labels['en'] ?? '').map((r) => r.name)).toEqual([
      null,
      'Project manager',
      null,
    ]);
    expect(withRef([{ id: 'g1', name: 'Quality' }], { id: 'g1', name: 'Quality' })).toHaveLength(1);
    expect(withoutRef([{ id: 'g1', name: 'Quality' }], 'g1')).toEqual([]);
  });
});

describe('NotificationsTab', () => {
  async function openNotifications() {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [{ path: 'admin/configurations/:id', component: ConfigPage, children: [{ path: 'notifications', component: NotificationsTab }] }],
          withComponentInputBinding(),
        ),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve));
        TestBed.tick();
      }
    };
    const reply = async (match: (url: string, method: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => match(r.urlWithParams, r.method));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no matching request');
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl(`/admin/configurations/${CONFIG.id}/notifications`);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    await reply((url, method) => url.endsWith('/notification-rules') && method === 'GET', RULES);
    const root = () => document.body;
    const tab = () => root().querySelector('app-notifications-tab')!;
    const dialog = () => root().querySelector('mat-dialog-container')!;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const texts = (selector: string, within: Element = tab()) => Array.from(within.querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    return { reply, root, tab, dialog, text, texts, click };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists whom each event notifies', async () => {
    const { tab, texts } = await openNotifications();
    expect(texts('td.event')).toEqual(['Ticket created', 'Ticket assigned', 'Status changed', 'Comment added', 'Someone mentioned', 'Absence conflict']);
    const rows = Array.from(tab().querySelectorAll('tr[mat-row]'));
    expect(texts('.chip', rows[0])).toEqual(['Reporter', 'badge Project manager', 'badge Deleted']);
    expect(texts('.none', rows[2])).toEqual(['Nobody']);
    expect(texts('.chip', rows[3])).toEqual(['Watchers', 'People mentioned', 'groups Quality', 'person Bob Martin']);
    // Absence conflicts (L08-US04): the assignee and the project managers.
    expect(texts('.chip', rows[5])).toEqual(['Assignee', 'badge Project manager']);
    expect(await axeViolations(tab())).toEqual([]);
  });

  it('sets "Ticket created" to notify the reporter and the project managers, and a group', async () => {
    const { tab, dialog, root, reply, click, text, texts } = await openNotifications();
    await click(tab().querySelectorAll('button.edit')[2]);
    await reply((url) => url.endsWith('/transition-roles'), REFERENCES.roles);
    expect(text(dialog().querySelector('h2'))).toBe('Whom “Status changed” notifies');
    // No "people mentioned" for a status change.
    expect(texts('mat-checkbox', dialog())).toEqual(['Reporter', 'Assignee', 'Watchers']);
    await click(dialog().querySelector('mat-checkbox input'));
    await click(dialog().querySelector('mat-select .mat-mdc-select-trigger'));
    await click(Array.from(root().querySelectorAll('mat-option')).find((o) => text(o) === 'Project manager'));
    await click(Array.from(root().querySelectorAll('.cdk-overlay-backdrop')).at(-1));
    const groups = dialog().querySelector('input.group-search') as HTMLInputElement;
    groups.focus();
    groups.dispatchEvent(new Event('focusin'));
    groups.value = 'qual';
    groups.dispatchEvent(new Event('input'));
    await reply((url) => url.includes('transition-candidates') && url.includes('search=qual'), [
      { kind: 'group', id: 'g1', name: 'Quality', email: null },
    ]);
    await click(root().querySelector('.mat-mdc-autocomplete-panel mat-option'));
    expect(texts('mat-chip', dialog())).toEqual(['Quality cancel']);
    expect(await axeViolations(dialog())).toEqual([]);

    await click(dialog().querySelector('button.confirm'));
    const saved = await reply((url, method) => url.endsWith('/notification-rules/status_changed') && method === 'PUT', {
      ...RULES,
      stage: 'draft',
    });
    expect(saved.request.headers.get('If-Match')).toBe('"1"');
    expect(saved.request.body).toEqual({ recipientKinds: ['reporter'], roleIds: ['r1'], groupIds: ['g1'], accountIds: [] });
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Notifications of “Status changed” saved in the draft.');
  });

  it('keeps the members of a rule and removes one', async () => {
    const { tab, dialog, reply, click, texts } = await openNotifications();
    await click(tab().querySelectorAll('button.edit')[3]);
    await reply((url) => url.endsWith('/transition-roles'), REFERENCES.roles);
    expect(texts('mat-checkbox', dialog())).toEqual(['Reporter', 'Assignee', 'Watchers', 'People mentioned']);
    expect(texts('mat-chip', dialog())).toEqual(['Quality cancel', 'Bob Martin cancel']);
    await click(dialog().querySelector('button[aria-label="Remove Bob Martin"]'));
    await click(dialog().querySelector('button.confirm'));
    const saved = await reply((url, method) => url.endsWith('/notification-rules/commented') && method === 'PUT', RULES);
    expect(saved.request.body).toEqual({
      recipientKinds: ['watchers', 'mentioned'],
      roleIds: [],
      groupIds: ['g1'],
      accountIds: [],
    });
  });
});
