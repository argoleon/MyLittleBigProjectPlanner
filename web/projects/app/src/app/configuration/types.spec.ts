/**
 * Tests of the Types tab of a configuration (L04-US02): the rules of the
 * hierarchy offered in the panel and the order, then the tab in the
 * configuration page with the API mocked at the HTTP level and the real
 * translations: the list, adding, changing and removing a type, the
 * refusals of the API, and the order changed with the keyboard.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, IssueType, IssueTypeList, ProjectConfig, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ConfigPage } from './config-page';
import { descendants, parentCandidates, reordered, typeFields } from './issue-types';
import { TypesTab } from './types-tab';

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

const type = (id: string, key: string, en: string, color: IssueType['color'], parentIds: string[], level: number, isSubtask = false): IssueType => ({
  id,
  key,
  label: en,
  labelTranslations: { fr: en, en },
  icon: { epic: 'bolt', story: 'bookmark', task: 'task_alt', bug: 'bug_report' }[color],
  color,
  isSubtask,
  parentIds,
  workflowId: 'w1',
  level,
  version: 1,
});

const EPIC = type('t1', 'epic', 'Epic', 'epic', [], 1);
const STORY = type('t2', 'story', 'Story', 'story', ['t1'], 2);
const TASK = type('t3', 'task', 'Task', 'task', ['t1'], 2);
const BUG = type('t4', 'bug', 'Bug', 'bug', ['t1'], 2);
const SUBTASK = { ...type('t5', 'subtask', 'Sub-task', 'task', ['t2', 't3', 't4'], 3, true), icon: 'subdirectory_arrow_right' };
const TYPES = [EPIC, STORY, TASK, BUG, SUBTASK];

/** The workflow the types follow, as the panel offers it. */
const WORKFLOW = {
  id: 'w1',
  key: 'standard',
  label: 'Standard workflow',
  labelTranslations: { fr: 'Workflow standard', en: 'Standard workflow' },
  issueTypeIds: ['t1', 't2', 't3', 't4', 't5'],
  statuses: [],
  transitions: [],
  problems: [],
  version: 1,
};

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'configuration_admin' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

function problem(status: number, code: string, errors: object[] = []) {
  return { type: `urn:mlbpp:problem:${code}`, title: code, status, code, errors };
}

describe('the rules of the Types tab', () => {
  it('finds the types under a type, at any depth', () => {
    expect([...descendants(TYPES, 't1')].sort()).toEqual(['t2', 't3', 't4', 't5']);
    expect([...descendants(TYPES, 't2')]).toEqual(['t5']);
    expect(descendants(TYPES, 't5').size).toBe(0);
  });

  it('offers as parents neither sub-tasks, nor the type itself, nor what is under it', () => {
    expect(parentCandidates(TYPES, null).map((t) => t.key)).toEqual(['epic', 'story', 'task', 'bug']);
    expect(parentCandidates(TYPES, 't2').map((t) => t.key)).toEqual(['epic', 'task', 'bug']);
    expect(parentCandidates(TYPES, 't1').map((t) => t.key)).toEqual([]);
  });

  it('orders the types once one moves', () => {
    expect(reordered(TYPES, 3, 0)).toEqual(['t4', 't1', 't2', 't3', 't5']);
    expect(reordered(TYPES, 0, 4)).toEqual(['t2', 't3', 't4', 't5', 't1']);
  });

  it('sends what the form holds, its key apart', () => {
    expect(
      typeFields({
        key: 'incident',
        labelTranslations: { en: 'Incident' },
        icon: 'emergency',
        color: 'bug',
        isSubtask: false,
        parentIds: ['t1'],
        workflowId: 'w1',
      }),
    ).toEqual({ labelTranslations: { en: 'Incident' }, icon: 'emergency', color: 'bug', isSubtask: false, parentIds: ['t1'], workflowId: 'w1' });
  });
});

describe('TypesTab', () => {
  async function openTypes(list: IssueTypeList = { stage: 'published', items: TYPES }) {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter(
          [{ path: 'admin/configurations/:id', component: ConfigPage, children: [{ path: 'types', component: TypesTab }] }],
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
    await harness.navigateByUrl(`/admin/configurations/${CONFIG.id}/types`);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, CONFIG);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}/issue-types`, list);
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}/workflows`, { stage: 'published', items: [WORKFLOW] });
    const root = () => document.body;
    const tab = () => root().querySelector('app-types-tab')!;
    const panel = () => root().querySelector('app-type-editor');
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const click = async (element: Element | null) => {
      (element as HTMLElement).click();
      await settle();
    };
    const fill = async (selector: string, value: string) => {
      const input = panel()!.querySelector(selector) as HTMLInputElement;
      input.value = value;
      input.dispatchEvent(new Event('input'));
      input.dispatchEvent(new Event('blur'));
      await settle();
    };
    /** The configuration read again after a change: its draft chip follows. */
    const configRead = (changes: number) =>
      reply((url) => url === `/api/v1/project-configs/${CONFIG.id}`, {
        ...CONFIG,
        draft: { number: 3, changeCount: changes, createdAt: '2026-11-13T08:00:00Z' },
      });
    const waitFor = async (done: () => boolean) => {
      for (let i = 0; i < 50 && !done(); i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        await settle();
      }
    };
    return { http, reply, settle, root, tab, panel, text, click, fill, configRead, waitFor };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the types with their identifier, allowed parents and level', async () => {
    const { tab, text } = await openTypes();
    const rows = Array.from(tab().querySelectorAll('tr[mat-row]'));
    const cells = rows.map((row) => Array.from(row.querySelectorAll('td')).slice(1, 5).map((cell) => text(cell)));
    expect(cells.map(([type]) => type)).toEqual([
      'boltType: Epic',
      'bookmarkType: Story',
      'task_altType: Task',
      'bug_reportType: Bug',
      'subdirectory_arrow_rightType: Sub-task Sub-task',
    ]);
    expect(cells.map(([, key, parents, level]) => [key, parents, level])).toEqual([
      ['epic', 'At the top', '1'],
      ['story', 'Epic', '2'],
      ['task', 'Epic', '2'],
      ['bug', 'Epic', '2'],
      ['subtask', 'Story, Task, Bug', '3'],
    ]);
    expect(rows[3].querySelector('.chip')?.getAttribute('data-kind')).toBe('bug');
    expect((rows[0].querySelector('button.up') as HTMLButtonElement).disabled).toBe(true);
    expect((rows[4].querySelector('button.down') as HTMLButtonElement).disabled).toBe(true);
    expect(rows[1].querySelector('button.edit')?.getAttribute('aria-label')).toBe('Edit Story');
    expect(await axeViolations(tab())).toEqual([]);
  });

  it('adds a type with its identifier, icon, color and parents, into the draft', async () => {
    const { tab, panel, reply, click, fill, text, configRead } = await openTypes();
    await click(tab().querySelector('button.add'));
    expect(text(panel()!.querySelector('h2'))).toBe('New issue type');
    // Parents offered: not the sub-task.
    const options = () => Array.from(document.querySelectorAll('mat-option')).map((o) => text(o));
    await click(panel()!.querySelector('mat-select[formcontrolname="parentIds"] .mat-mdc-select-trigger'));
    expect(options()).toEqual(['Epic', 'Story', 'Task', 'Bug']);
    await click(Array.from(document.querySelectorAll('mat-option')).find((o) => text(o) === 'Epic')!);
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));

    // A bad identifier and no label: said on the fields.
    await fill('input[formcontrolname="key"]', 'Bad-Key');
    await click(panel()!.querySelector('button.save'));
    const errors = Array.from(panel()!.querySelectorAll('mat-error')).map((e) => text(e));
    expect(errors).toEqual(['Required in Français, the fallback language.', 'A lowercase letter, then lowercase letters, digits or _.']);
    expect(await axeViolations(panel()!)).toEqual([]);

    await fill('input[lang="fr"]', 'Incident');
    await fill('input[lang="en"]', 'Incident');
    await fill('input[formcontrolname="key"]', 'incident');
    await click(panel()!.querySelector('mat-select[formcontrolname="icon"] .mat-mdc-select-trigger'));
    await click(Array.from(document.querySelectorAll('mat-option')).find((o) => text(o)?.endsWith('emergency'))!);
    await click(panel()!.querySelector('mat-select[formcontrolname="color"] .mat-mdc-select-trigger'));
    await click(Array.from(document.querySelectorAll('mat-option')).find((o) => text(o) === 'Bug color')!);
    await click(panel()!.querySelector('button.save'));
    const incident = type('t6', 'incident', 'Incident', 'bug', ['t1'], 2);
    const created = await reply(
      (url, method) => url === `/api/v1/project-configs/${CONFIG.id}/issue-types` && method === 'POST',
      { ...incident, icon: 'emergency' },
      201,
    );
    expect(created.request.body).toEqual({
      key: 'incident',
      labelTranslations: { fr: 'Incident', en: 'Incident' },
      icon: 'emergency',
      color: 'bug',
      isSubtask: false,
      parentIds: ['t1'],
      workflowId: 'w1',
    });
    expect(created.request.headers.get('Idempotency-Key')).toBeTruthy();
    expect(text(tab().querySelector('[role="status"]'))).toBe('Type “Incident” saved in the draft.');
    // The list and the draft chip follow.
    await reply((url) => url === `/api/v1/project-configs/${CONFIG.id}/issue-types`, {
      stage: 'draft',
      items: [...TYPES, { ...incident, icon: 'emergency' }],
    });
    await configRead(1);
    expect(tab().querySelectorAll('tr[mat-row]').length).toBe(6);
    expect(text(document.querySelector('app-config-page app-config-chips'))).toContain('Unpublished draft · 1 change');
  });

  it('changes a type with the version it read, its key fixed, and says why the API refuses', async () => {
    const { tab, panel, reply, click, fill, text } = await openTypes();
    await click(tab().querySelectorAll('button.edit')[1]);
    expect(text(panel()!.querySelector('h2'))).toBe('Edit “Story”');
    expect((panel()!.querySelector('input[formcontrolname="key"]') as HTMLInputElement).disabled).toBe(true);
    expect(text(panel()!.querySelector('mat-hint'))).toContain('Fallback language');
    // Parents offered: neither itself nor what is under it.
    await click(panel()!.querySelector('mat-select[formcontrolname="parentIds"] .mat-mdc-select-trigger'));
    expect(Array.from(document.querySelectorAll('mat-option')).map((o) => text(o))).toEqual(['Epic', 'Task', 'Bug']);
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));

    await fill('input[lang="en"]', 'User story');
    await click(panel()!.querySelector('button.save'));
    const refused = await reply((url, method) => url.endsWith('/issue-types/t2') && method === 'PUT', problem(409, 'issue_type.cycle'), 409);
    expect(refused.request.headers.get('If-Match')).toBe('"1"');
    expect(refused.request.body).toEqual({
      labelTranslations: { fr: 'Story', en: 'User story' },
      icon: 'bookmark',
      color: 'story',
      isSubtask: false,
      parentIds: ['t1'],
      workflowId: 'w1',
    });
    expect(text(panel()!.querySelector('[role="alert"]'))).toBe('A type cannot be its own ancestor.');

    // A sub-task needs a parent type.
    await click(tab().querySelectorAll('button.edit')[4]);
    await click(panel()!.querySelector('mat-select[formcontrolname="parentIds"] .mat-mdc-select-trigger'));
    for (const label of ['Story', 'Task', 'Bug']) {
      await click(Array.from(document.querySelectorAll('mat-option')).find((o) => text(o) === label)!);
    }
    document.body.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    await click(panel()!.querySelector('button.save'));
    expect(text(panel()!.querySelector('mat-error'))).toBe('A sub-task type needs at least one parent type.');
  });

  it('removes a type after a confirmation, and says when it must stay', async () => {
    const { tab, panel, reply, click, text, configRead, waitFor } = await openTypes();
    const confirm = () => Array.from(document.querySelectorAll('mat-dialog-container button')).find((b) => text(b) === 'Remove')!;
    await click(tab().querySelectorAll('button.edit')[0]);
    await click(panel()!.querySelector('button.remove'));
    expect(text(document.querySelector('mat-dialog-container h2'))).toBe('Remove “Epic”?');
    await click(confirm());
    await reply((url, method) => url.endsWith('/issue-types/t1') && method === 'DELETE', problem(409, 'issue_type.last_type'), 409);
    expect(text(panel()!.querySelector('[role="alert"]'))).toBe('A configuration keeps at least one type that is not a sub-task.');

    await click(tab().querySelectorAll('button.edit')[3]);
    await click(panel()!.querySelector('button.remove'));
    await waitFor(() => confirm() !== undefined);
    await click(confirm());
    const removed = await reply((url, method) => url.endsWith('/issue-types/t4') && method === 'DELETE', null, 204);
    expect(removed.request.headers.get('If-Match')).toBe('"1"');
    await waitFor(() => panel() === null);
    expect(panel()).toBeNull();
    expect(text(tab().querySelector('[role="status"]'))).toBe('Type “Bug” removed from the draft.');
    await reply((url) => url.endsWith('/issue-types'), { stage: 'draft', items: [EPIC, STORY, TASK, { ...SUBTASK, parentIds: ['t2', 't3'] }] });
    await configRead(2);
    expect(tab().querySelectorAll('tr[mat-row]').length).toBe(4);
  });

  it('moves a type with the keyboard, without dragging', async () => {
    const { tab, reply, click, text, configRead } = await openTypes();
    const down = tab().querySelectorAll('button.down')[0] as HTMLButtonElement;
    expect(down.getAttribute('aria-label')).toBe('Move Epic down');
    down.focus();
    await click(down);
    const moved = await reply(
      (url, method) => url.endsWith('/issue-types/order') && method === 'PUT',
      { stage: 'draft', items: [STORY, EPIC, TASK, BUG, SUBTASK] },
    );
    expect(moved.request.body).toEqual({ ids: ['t2', 't1', 't3', 't4', 't5'] });
    await configRead(2);
    expect(text(tab().querySelector('[role="status"]'))).toBe('Epic moved to position 2.');
    const keys = Array.from(tab().querySelectorAll('code.key')).map((k) => text(k));
    expect(keys).toEqual(['story', 'epic', 'task', 'bug', 'subtask']);
  });
});
