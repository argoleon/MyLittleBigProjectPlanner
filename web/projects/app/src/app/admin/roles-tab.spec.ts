/**
 * Integration tests of the "Roles" tab of the administration, with the API
 * mocked at the HTTP level and the real translations.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { Role, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { AdminPage } from './admin-page';
import { RolesTab } from './roles-tab';

const READER: Role = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b01',
  label: 'Lecteur',
  labelTranslations: { fr: 'Lecteur', en: 'Reader' },
  permissions: ['view'],
  grantCount: 0,
  version: 1,
};

const MANAGER: Role = {
  id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b02',
  label: 'Chef de projet',
  labelTranslations: { fr: 'Chef de projet', en: 'Project manager' },
  permissions: ['view', 'create_ticket', 'edit', 'comment', 'log_time', 'assign_resources', 'force_overload', 'manage_sprints'],
  grantCount: 3,
  version: 2,
};

const PERMISSION_LABELS = [
  'View',
  'Create a ticket',
  'Edit',
  'Comment',
  'Log time',
  'Assign resources',
  'Force an overload',
  'Manage sprints',
  'Manage members',
  'Administer the node',
];

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

async function open(roles: Role[] = [MANAGER, READER]) {
  localStorage.clear();
  TestBed.configureTestingModule({
    providers: [
      provideRouter([{ path: 'admin', component: AdminPage, children: [{ path: 'roles', component: RolesTab }] }]),
      provideApi(),
      provideHttpClientTesting(),
      provideI18nTesting(),
      provideMaterialDefaults(),
    ],
  });
  const harness = await RouterTestingHarness.create();
  await harness.navigateByUrl('/admin/roles');
  const http = TestBed.inject(HttpTestingController);
  const settle = async () => {
    for (let i = 0; i < 3; i++) {
      await new Promise((resolve) => setTimeout(resolve));
      TestBed.tick();
    }
  };
  const reply = async (match: (url: string) => boolean, body: object | null, status = 200): Promise<TestRequest> => {
    let request: TestRequest | undefined;
    for (let i = 0; i < 50 && !request; i++) {
      [request] = http.match((r) => match(r.urlWithParams));
      if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
    }
    if (!request) throw new Error('no matching request');
    request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
    await settle();
    return request;
  };
  const isList = (url: string) => url.startsWith('/api/v1/tenant/roles?');
  await reply((url) => url === '/api/v1/tenant', {
    id: 't',
    slug: 'acme',
    name: 'Acme Industrie',
    defaultLocale: 'fr',
    timeZone: 'Europe/Paris',
  });
  await reply(isList, { items: roles, nextCursor: null });
  const root = () => document.body;
  const editor = () => root().querySelector('app-role-editor');
  const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
  const click = async (element: Element | null) => {
    (element as HTMLElement).click();
    await settle();
  };
  const type = async (selector: string, value: string) => {
    const input = editor()!.querySelector(selector) as HTMLInputElement;
    input.value = value;
    input.dispatchEvent(new Event('input'));
    await settle();
  };
  const toggle = (label: string) =>
    Array.from(root().querySelectorAll('mat-slide-toggle')).find((t) => text(t.querySelector('label')) === label)!
      .querySelector('button[role="switch"]') as HTMLButtonElement;
  const checked = () =>
    Array.from(root().querySelectorAll('mat-slide-toggle'))
      .filter((t) => t.querySelector('button')?.getAttribute('aria-checked') === 'true')
      .map((t) => text(t.querySelector('label')));
  const button = (cls: string) => editor()!.querySelector(`button.${cls}`) as HTMLButtonElement;
  // The last one opened: a closed one may still be leaving.
  const dialog = () => Array.from(root().querySelectorAll('mat-dialog-container')).at(-1)!;
  /** Clicks button `index` of the open dialog and waits until it is gone. */
  const answer = async (index: number) => {
    const container = dialog();
    await click(container.querySelectorAll('button')[index]);
    for (let i = 0; i < 50 && container.isConnected; i++) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    await settle();
  };
  const listItem = (label: string) =>
    Array.from(root().querySelectorAll('mat-list-option')).find((o) => o.textContent?.includes(label))!;
  return { reply, settle, root, editor, text, click, type, toggle, checked, button, dialog, answer, listItem, isList };
}

describe('RolesTab', () => {
  afterEach(() => {
    TestBed.inject(MatDialog).closeAll();
    TestBed.inject(HttpTestingController).verify();
  });

  it('lists the roles and shows the first one with a switch per permission', async () => {
    const { root, editor, text, checked, listItem } = await open();
    expect(text(listItem('Project manager'))).toBe('Project manager 8 / 10 permissions · Grants: 3');
    expect(root().querySelector('mat-selection-list')!.getAttribute('aria-label')).toBe('Roles of the organization');
    expect(listItem('Project manager').getAttribute('aria-selected')).toBe('true');
    expect(listItem('Reader').getAttribute('aria-selected')).toBe('false');
    expect(text(editor()?.querySelector('h2'))).toBe('Role “Project manager”');
    expect((editor()!.querySelector('input[lang="fr"]') as HTMLInputElement).value).toBe('Chef de projet');
    expect((editor()!.querySelector('input[lang="en"]') as HTMLInputElement).value).toBe('Project manager');
    // Every switch is named by its permission.
    const switches = Array.from(root().querySelectorAll('mat-slide-toggle'));
    expect(switches.map((s) => text(s.querySelector('label')))).toEqual(PERMISSION_LABELS);
    for (const s of switches) {
      const id = s.querySelector('button[role="switch"]')!.id;
      expect(s.querySelector(`label[for="${id}"]`)).not.toBeNull();
    }
    expect(checked()).toEqual(PERMISSION_LABELS.slice(0, 8));
    expect(await axeViolations(root().querySelector('app-admin-page')!)).toEqual([]);
  });

  it('tracks unsaved changes and discards them', async () => {
    const { editor, text, click, toggle, checked, button } = await open();
    expect(editor()!.querySelector('.unsaved')).toBeNull();
    expect(button('discard').disabled).toBe(true);
    await click(toggle('Manage members'));
    expect(text(editor()!.querySelector('.unsaved'))).toBe('Unsaved changes');
    // Switched back: nothing to save any more.
    await click(toggle('Manage members'));
    expect(editor()!.querySelector('.unsaved')).toBeNull();
    await click(toggle('Comment'));
    expect(checked()).not.toContain('Comment');
    await click(button('discard'));
    expect(checked()).toContain('Comment');
    expect(editor()!.querySelector('.unsaved')).toBeNull();
  });

  it('saves the changed permissions with the version of the role', async () => {
    const { root, editor, text, click, type, toggle, button, reply, isList } = await open();
    await click(toggle('Manage members'));
    await type('input[lang="en"]', ' Manager ');
    await click(button('save'));
    const saved: Role = {
      ...MANAGER,
      labelTranslations: { fr: 'Chef de projet', en: 'Manager' },
      permissions: [...MANAGER.permissions, 'manage_members'],
      version: 3,
    };
    const request = await reply((url) => url === `/api/v1/tenant/roles/${MANAGER.id}`, saved);
    expect(request.request.method).toBe('PUT');
    expect(request.request.headers.get('If-Match')).toBe('"2"');
    expect(request.request.body).toEqual({
      labelTranslations: { fr: 'Chef de projet', en: 'Manager' },
      permissions: [...MANAGER.permissions, 'manage_members'],
    });
    await reply(isList, { items: [saved, READER], nextCursor: null });
    expect(text(root().querySelector('[role="status"]'))).toBe('Role Manager saved.');
    expect(text(editor()?.querySelector('h2'))).toBe('Role “Manager”');
    expect(editor()!.querySelector('.unsaved')).toBeNull();
  });

  it('says when someone changed the role in the meantime', async () => {
    const { editor, text, click, toggle, button, reply } = await open();
    await click(toggle('Manage members'));
    await click(button('save'));
    await reply((url) => url === `/api/v1/tenant/roles/${MANAGER.id}`, problem(412, 'precondition_failed'), 412);
    expect(text(editor()!.querySelector('[role="alert"]'))).toBe(
      'Someone changed this item in the meantime. Reload it before trying again.',
    );
    expect(text(editor()!.querySelector('.unsaved'))).toBe('Unsaved changes');
  });

  it('creates a role, checking its label and permissions first', async () => {
    const { root, editor, text, click, type, toggle, button, reply, isList } = await open();
    await click(root().querySelector('button.add'));
    expect(text(editor()?.querySelector('h2'))).toBe('New role');
    expect(editor()!.querySelector('button.delete')).toBeNull();
    // Nothing filled in: nothing sent.
    await click(button('save'));
    expect(text(editor()!.querySelector('mat-error'))).toBe('This field is required.');
    expect(text(editor()!.querySelector('#role-permissions-error'))).toBe('Choose at least one permission.');
    expect(editor()!.querySelector('fieldset')!.getAttribute('aria-describedby')).toBe('role-permissions-error');

    await type('input[lang="en"]', 'Reader');
    await click(toggle('View'));
    expect(editor()!.querySelector('#role-permissions-error')).toBeNull();
    await click(button('save'));
    // The label is taken: said on the field.
    await reply(
      (url) => url === '/api/v1/tenant/roles',
      problem(409, 'role.label_taken', [{ field: 'labelTranslations', code: 'taken' }]),
      409,
    );
    expect(text(editor()!.querySelector('mat-error'))).toBe('This value is already used.');

    await type('input[lang="en"]', 'Guest');
    await type('input[lang="fr"]', 'Invité');
    await click(button('save'));
    const created: Role = {
      id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b03',
      label: 'Invité',
      labelTranslations: { fr: 'Invité', en: 'Guest' },
      permissions: ['view'],
      grantCount: 0,
      version: 1,
    };
    const request = await reply((url) => url === '/api/v1/tenant/roles', created, 201);
    expect(request.request.method).toBe('POST');
    expect(request.request.headers.get('Idempotency-Key')).toMatch(/^[0-9a-f-]{36}$/);
    expect(request.request.body).toEqual({ labelTranslations: { fr: 'Invité', en: 'Guest' }, permissions: ['view'] });
    await reply(isList, { items: [MANAGER, READER, created], nextCursor: null });
    expect(text(root().querySelector('[role="status"]'))).toBe('Role Guest saved.');
    expect(text(editor()?.querySelector('h2'))).toBe('Role “Guest”');
  });

  it('duplicates a role with its permissions, under a new label', async () => {
    const { editor, text, click, type, checked, button, reply, isList } = await open();
    await click(button('duplicate'));
    expect(text(editor()?.querySelector('h2'))).toBe('Copy of “Project manager”');
    expect((editor()!.querySelector('input[lang="en"]') as HTMLInputElement).value).toBe('');
    expect(checked()).toEqual(PERMISSION_LABELS.slice(0, 8));
    await type('input[lang="en"]', 'Team lead');
    await click(button('save'));
    const created: Role = { ...MANAGER, id: '0194f0a2-7c1e-7d3a-9b2f-3c4d5e6f7b04', labelTranslations: { en: 'Team lead' } };
    const request = await reply((url) => url === '/api/v1/tenant/roles', created, 201);
    expect(request.request.body).toEqual({ labelTranslations: { en: 'Team lead' }, permissions: MANAGER.permissions });
    await reply(isList, { items: [MANAGER, READER, created], nextCursor: null });
  });

  it('asks before leaving a role with unsaved changes', async () => {
    const { editor, text, click, toggle, dialog, answer, listItem } = await open();
    await click(toggle('Manage members'));
    await click(listItem('Reader'));
    expect(text(dialog().querySelector('h2'))).toBe('Discard the changes?');
    // Cancelled: the changes stay.
    await answer(0);
    expect(text(editor()?.querySelector('h2'))).toBe('Role “Project manager”');
    expect(text(editor()!.querySelector('.unsaved'))).toBe('Unsaved changes');
    expect(listItem('Project manager').getAttribute('aria-selected')).toBe('true');
    expect(listItem('Reader').getAttribute('aria-selected')).toBe('false');
    await click(listItem('Reader'));
    await answer(1);
    expect(listItem('Reader').getAttribute('aria-selected')).toBe('true');
    expect(text(editor()?.querySelector('h2'))).toBe('Role “Reader”');
    expect(editor()!.querySelector('.unsaved')).toBeNull();
  });

  it('refuses to delete a role used by grants, and deletes an unused one after confirmation', async () => {
    const { editor, text, click, button, dialog, answer, listItem, reply, isList } = await open();
    expect(button('delete').disabled).toBe(true);
    expect(text(editor()!.querySelector('.in-use'))).toBe('Grants using this role: 3. Remove them before deleting it.');

    await click(listItem('Reader'));
    expect(editor()!.querySelector('.in-use')).toBeNull();
    await click(button('delete'));
    expect(text(dialog().querySelector('h2'))).toBe('Delete Reader?');
    await answer(1);
    const deleted = await reply((url) => url === `/api/v1/tenant/roles/${READER.id}`, null, 204);
    expect(deleted.request.method).toBe('DELETE');
    expect(deleted.request.headers.get('If-Match')).toBe('"1"');
    await reply(isList, { items: [MANAGER], nextCursor: null });
    expect(text(document.body.querySelector('[role="status"]'))).toBe('Role Reader deleted.');
    expect(text(editor()?.querySelector('h2'))).toBe('Role “Project manager”');
  });

  it('says when grants appeared since the list was loaded', async () => {
    const { editor, text, click, button, answer, listItem, reply } = await open();
    await click(listItem('Reader'));
    await click(button('delete'));
    await answer(1);
    await reply(
      (url) => url === `/api/v1/tenant/roles/${READER.id}`,
      { ...problem(409, 'role.in_use'), detail: 'used by 1 grants' },
      409,
    );
    expect(text(editor()!.querySelector('[role="alert"]'))).toBe('Grants use this role: remove them first.');
  });

  it('shows an empty state when the tenant has no role', async () => {
    const { root, text, click, editor } = await open([]);
    expect(text(root().querySelector('ui-empty-state [role="heading"]'))).toBe('No role yet');
    expect(editor()).toBeNull();
    await click(root().querySelector('button.add'));
    expect(text(editor()?.querySelector('h2'))).toBe('New role');
  });
});
