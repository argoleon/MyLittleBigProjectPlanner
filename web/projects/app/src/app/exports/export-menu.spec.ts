/**
 * Tests of the Export menu of list views (L06-US06), the API mocked at the
 * HTTP level and the real translations: the query and displayed columns
 * sent, a small export downloaded at once, a large one announced with a
 * link to follow it, refusals told, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter } from '@angular/router';
import { ListExport, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { ExportMenu } from './export-menu';
import { FileDownload } from './file-download';

function exported(status: ListExport['status'], overrides: Partial<ListExport> = {}): ListExport {
  return {
    id: 'e1',
    fileName: 'search-2026-11-18.csv',
    format: 'csv',
    status,
    rows: status === 'ready' ? 12 : null,
    sizeBytes: null,
    failure: null,
    downloadPath: status === 'ready' ? '/api/v1/exports/e1/file' : null,
    createdAt: '2026-11-18T09:00:00Z',
    completedAt: null,
    expiresAt: '2026-11-19T09:00:00Z',
    ...overrides,
  };
}

describe('the Export menu', () => {
  async function open(disabled = false) {
    const downloads: string[] = [];
    TestBed.configureTestingModule({
      providers: [
        provideRouter([]),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
        { provide: FileDownload, useValue: { start: (path: string) => downloads.push(path) } },
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        TestBed.tick();
      }
    };
    const fixture = TestBed.createComponent(ExportMenu);
    fixture.componentRef.setInput('query', 'project = PLAT ORDER BY priority DESC');
    fixture.componentRef.setInput('columns', ['key', 'summary', 'status', 'assignee']);
    fixture.componentRef.setInput('name', 'search');
    fixture.componentRef.setInput('disabled', disabled);
    document.body.appendChild(fixture.nativeElement);
    await settle();
    const root = () => fixture.nativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    const exportCsv = async () => {
      (root().querySelector('button.export') as HTMLElement).click();
      await settle();
      (document.querySelector('button.csv') as HTMLElement).click();
      await settle();
    };
    const reply = async (body: object, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => r.url === '/api/v1/exports' && r.method === 'POST');
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no export asked');
      request.flush(body, { status, statusText: 'S', headers: status >= 400 ? { 'content-type': 'application/problem+json' } : {} });
      await settle();
      return request;
    };
    return { root, text, exportCsv, reply, downloads, settle };
  }

  it('sends the query and the displayed columns, and downloads a small export at once', async () => {
    const { root, text, exportCsv, reply, downloads } = await open();
    expect(text(root().querySelector('button.export'))).toBe('download Export');
    await exportCsv();
    const asked = await reply(exported('ready'), 201);
    expect(asked.request.body).toEqual({
      query: 'project = PLAT ORDER BY priority DESC',
      columns: ['key', 'summary', 'status', 'assignee'],
      format: 'csv',
      name: 'search',
    });
    expect(downloads).toEqual(['/api/v1/exports/e1/file']);
    expect(root().querySelector('[role="status"]')).toBeNull();
  });

  it('announces a large export, with a link to follow it', async () => {
    const { root, text, exportCsv, reply, downloads } = await open();
    await exportCsv();
    await reply(exported('queued'), 202);
    expect(downloads).toEqual([]);
    expect(text(root().querySelector('[role="status"]'))).toBe(
      'search-2026-11-18.csv is being prepared: you will be notified when it is ready. Follow it',
    );
    expect(root().querySelector('[role="status"] a')?.getAttribute('href')).toBe('/exports/e1');
    expect(await axeViolations(root())).toEqual([]);
  });

  it('tells why an export is refused', async () => {
    const { root, text, exportCsv, reply } = await open();
    await exportCsv();
    await reply({ code: 'export.too_large', status: 422, title: 'Too large' }, 422);
    expect(text(root().querySelector('[role="alert"]'))).toBe('This list holds more than 100,000 tickets: narrow the query to export it.');
    await exportCsv();
    await reply({ code: 'query.invalid', status: 400, title: 'Invalid', query: { reason: 'syntax', position: 1, length: 0, found: null } }, 400);
    expect(text(root().querySelector('[role="alert"]'))).toBe('The query cannot run: correct it before exporting.');
    await exportCsv();
    await reply({ code: 'storage.unavailable', status: 503, title: 'Unavailable' }, 503);
    expect(root().querySelector('[role="alert"]')).not.toBeNull();
  });

  it('stays closed while the list cannot be exported', async () => {
    const { root } = await open(true);
    expect((root().querySelector('button.export') as HTMLButtonElement).disabled).toBe(true);
  });
});
