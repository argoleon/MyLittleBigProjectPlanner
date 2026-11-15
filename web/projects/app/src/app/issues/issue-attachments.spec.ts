/**
 * Tests of the attachments of the ticket page (L05-US05), with the API and
 * the storage mocked at the HTTP level and the real translations: the files
 * with their thumbnail, download link, size, uploader and scan status; a
 * dropped file sent with its progress, then listed and read again while it
 * is scanned; a refusal said; a removal once confirmed; readers who only
 * read; accessibility.
 */
import { HttpBackend, HttpEventType } from '@angular/common/http';
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';
import { By } from '@angular/platform-browser';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { AuthService, Issue, IssueAttachment, UPLOAD_BACKEND, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { IssueAttachments, SCAN_POLL_MS } from './issue-attachments';
import { IssuePage } from './issue-page';

const SESSION = {
  account: { id: 'a1', email: 'lucas@acme.example', displayName: 'Lucas Bernard', locale: null, tenantRole: 'member' },
  stage: 'complete',
  csrfToken: 'csrf',
  expiresAt: '2026-11-20T09:00:00Z',
};

const ISSUE: Issue = {
  id: 'i142',
  key: 'PLAT-142',
  project: { id: 'p1', key: 'PLAT', name: 'Identity platform' },
  type: { id: 't', key: 'story', label: 'Story', labelTranslations: { en: 'Story' }, icon: 'bookmark', color: 'story', isSubtask: false, parentIds: [] },
  status: { id: 's1', key: 'todo', label: 'To do', category: 'todo' },
  summary: 'Migrate the authentication to OIDC',
  description: null,
  descriptionHtml: null,
  priority: 'medium',
  reporter: null,
  assignee: null,
  startDate: null,
  dueDate: null,
  fields: [],
  parentId: null,
  children: [],
  links: [],
  createdAt: '2026-11-06T09:12:00Z',
  updatedAt: '2026-11-06T18:20:00Z',
  version: 4,
};

const file = (id: string, rest: Partial<IssueAttachment> = {}): IssueAttachment => ({
  id,
  name: `${id}.pdf`,
  size: 3_145_728,
  mediaType: 'application/pdf',
  status: 'clean',
  signature: null,
  uploadedBy: { id: 'a1', name: 'Lucas Bernard' },
  createdAt: '2026-11-10T09:00:00Z',
  hasThumbnail: false,
  canDelete: false,
  ...rest,
});

const FILES: IssueAttachment[] = [
  file('plan', { canDelete: true }),
  file('screen', { name: 'screen.png', mediaType: 'image/png', size: 48_000, hasThumbnail: true }),
  file('invoice', { name: 'invoice.exe', mediaType: 'application/octet-stream', status: 'infected', signature: 'Eicar-Test-Signature' }),
];

describe('attachments on the ticket page', () => {
  async function openTicket(permissions: string[] = ['view', 'edit'], files: IssueAttachment[] = FILES) {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'issues/:key', component: IssuePage }], withComponentInputBinding()),
        provideApi(),
        provideHttpClientTesting(),
        { provide: UPLOAD_BACKEND, useExisting: HttpBackend },
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
    const next = async (match: (url: string, method: string) => boolean): Promise<TestRequest> => {
      for (let i = 0; i < 50; i++) {
        const [request] = http.match((r) => match(r.urlWithParams, r.method));
        if (request) return request;
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      throw new Error('no matching request');
    };
    const reply = async (match: (url: string, method: string) => boolean, body: object | string | null, status = 200) => {
      const request = await next(match);
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const isList = (url: string, method: string) => url === '/api/v1/issues/i142/attachments' && method === 'GET';
    const loaded = TestBed.inject(AuthService).load();
    await reply((url) => url === '/api/v1/auth/session', SESSION);
    await loaded;
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/issues/PLAT-142');
    await reply((url, method) => url.startsWith('/api/v1/issues/PLAT-142') && method === 'GET', ISSUE);
    await reply((url) => url === '/api/v1/nodes/p1/my-permissions', permissions);
    await reply(isList, files);
    const root = () => document.body;
    const section = () => root().querySelector('app-issue-attachments')!;
    const component = () => harness.fixture.debugElement.query(By.directive(IssueAttachments)).componentInstance as IssueAttachments;
    const text = (element: Element | null | undefined) => {
      const copy = element?.cloneNode(true) as Element | undefined;
      copy?.querySelectorAll('mat-icon').forEach((icon) => icon.remove());
      return copy?.textContent?.replace(/\s+/g, ' ').trim();
    };
    const texts = (selector: string) => Array.from(section().querySelectorAll(selector)).map((e) => text(e));
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    return { http, next, reply, settle, root, section, component, text, texts, click, isList };
  }

  afterEach(() => TestBed.inject(MatDialog).closeAll());

  it('lists the files with their thumbnail, link, size, uploader and scan status', async () => {
    const { section, texts, text } = await openTicket();
    expect(text(section().querySelector('h2'))).toBe('Attachments (3)');
    expect(texts('.file .name')).toEqual(['plan.pdf', 'screen.png', 'invoice.exe']);
    const links = Array.from(section().querySelectorAll<HTMLAnchorElement>('a.name'));
    expect(links.map((a) => a.getAttribute('href'))).toEqual([
      '/api/v1/issues/i142/attachments/plan/content',
      '/api/v1/issues/i142/attachments/screen/content',
    ]);
    expect(links.every((a) => a.hasAttribute('download'))).toBe(true);
    expect(section().querySelector('img.thumbnail')?.getAttribute('src')).toBe('/api/v1/issues/i142/attachments/screen/thumbnail');
    expect(texts('.file .scan')).toEqual(['Clean', 'Clean', 'Infected']);
    expect(text(section().querySelector('.file .meta'))).toMatch(/^3\.1 MB · Lucas Bernard · Nov 10, 2026, \d{1,2}:\d{2}\s[AP]M$/);
    expect(texts('[data-status="infected"] .meta').at(-1)).toBe('Virus found: Eicar-Test-Signature');
    expect(Array.from(section().querySelectorAll('button.remove')).map((b) => b.getAttribute('aria-label'))).toEqual(['Remove the file plan.pdf']);
    expect(await axeViolations(section())).toEqual([]);
  });

  it('sends a dropped file with its progress, then reads the list again while it is scanned', async () => {
    const { section, component, next, reply, isList, texts, settle } = await openTicket();
    const changed = vi.fn();
    component().changed.subscribe(changed);
    const dropped = new File(['hello'], 'notes.txt', { type: 'text/plain' });
    const sending = component().send([dropped]);
    const announce = await next((url, method) => url === '/api/v1/issues/i142/attachments' && method === 'POST');
    expect(announce.request.body).toEqual({ name: 'notes.txt', size: 5, mediaType: 'text/plain' });
    announce.flush({ attachment: file('notes', { status: 'pending' }), uploadUrl: '/mlbpp-attachments/t/uploads/notes?sig=1' }, { status: 201, statusText: 'Created' });
    const put = await next((url, method) => url.startsWith('/mlbpp-attachments/') && method === 'PUT');
    put.event({ type: HttpEventType.UploadProgress, loaded: 2, total: 5 });
    await settle();
    expect(texts('.upload .name')).toEqual(['notes.txt']);
    expect(section().querySelector('.upload mat-progress-bar')?.getAttribute('aria-valuenow')).toBe('40');
    put.flush('', { status: 200, statusText: 'OK' });
    await reply((url, method) => url.endsWith('/attachments/notes/uploaded') && method === 'POST', file('notes', { status: 'scanning' }));
    await sending;
    expect(changed).toHaveBeenCalledTimes(1);
    await reply(isList, [...FILES, file('notes', { name: 'notes.txt', status: 'scanning' })]);
    expect(section().querySelector('.upload')).toBeNull();
    expect(texts('.file .scan').at(-1)).toBe('Scanning');
    // Read again once the scan had time to run.
    await new Promise((resolve) => setTimeout(resolve, SCAN_POLL_MS + 100));
    await reply(isList, [...FILES, file('notes', { name: 'notes.txt', status: 'clean' })]);
    expect(texts('.file .scan').at(-1)).toBe('Clean');
  });

  it('says why a file was refused, and lets it be dismissed', async () => {
    const { section, component, reply, text, click } = await openTicket();
    void component().send([new File(['x'], 'video.mp4')]);
    await reply(
      (url, method) => url === '/api/v1/issues/i142/attachments' && method === 'POST',
      { type: 'about:blank', title: 'Too large', status: 413, code: 'attachment.too_large' },
      413,
    );
    expect(text(section().querySelector('.upload [role="alert"]'))).toBe('The file is larger than the maximum allowed.');
    expect(await axeViolations(section())).toEqual([]);
    await click(section().querySelector('button.dismiss'));
    expect(section().querySelector('.upload')).toBeNull();
  });

  it('removes a file once confirmed', async () => {
    const { section, root, click, reply, isList, text } = await openTicket();
    await click(section().querySelector('button.remove'));
    const confirm = root().querySelector('ui-confirm-dialog')!;
    expect(text(confirm.querySelector('h2'))).toBe('Remove the file?');
    expect(text(confirm)).toContain('plan.pdf is removed from the ticket');
    await click(Array.from(confirm.querySelectorAll('button')).find((b) => text(b) === 'Remove'));
    await reply((url, method) => url === '/api/v1/issues/i142/attachments/plan' && method === 'DELETE', null, 204);
    await reply(isList, FILES.slice(1));
    expect(section().querySelectorAll('.file')).toHaveLength(2);
  });

  it('lets readers read and download, not attach', async () => {
    const { section } = await openTicket(['view'], []);
    expect(section().querySelector('.drop')).toBeNull();
    expect(section().querySelector('.empty')?.textContent?.trim()).toBe('No file attached.');
  });
});
