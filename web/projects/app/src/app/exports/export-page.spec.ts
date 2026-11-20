/**
 * Tests of the page of an export (L06-US06), where its notification leads:
 * ready with its download link, being prepared then read again, failed
 * with the reason, gone once expired, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { ListExport, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { EXPORT_POLL_MS, ExportPage } from './export-page';

function exported(status: ListExport['status'], overrides: Partial<ListExport> = {}): ListExport {
  return {
    id: 'e1',
    fileName: 'my-tickets-2026-11-18.csv',
    format: 'csv',
    status,
    rows: status === 'ready' ? 20001 : null,
    sizeBytes: null,
    failure: null,
    downloadPath: status === 'ready' ? '/api/v1/exports/e1/file' : null,
    createdAt: '2026-11-18T09:00:00Z',
    completedAt: null,
    expiresAt: '2026-11-19T09:00:00Z',
    ...overrides,
  };
}

describe('the page of an export', () => {
  async function open(first: object, status = 200) {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'exports/:id', component: ExportPage }], withComponentInputBinding()),
        provideApi(),
        provideHttpClientTesting(),
        provideI18nTesting(),
        provideMaterialDefaults(),
      ],
    });
    const http = TestBed.inject(HttpTestingController);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, 20));
        TestBed.tick();
      }
    };
    const reply = async (body: object, code = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => r.url === '/api/v1/exports/e1');
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no export read');
      request.flush(body, { status: code, statusText: 'S', headers: code >= 400 ? { 'content-type': 'application/problem+json' } : {} });
      await settle();
      return request;
    };
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/exports/e1');
    await reply(first, status);
    const page = () => harness.routeNativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    return { page, text, reply, http };
  }

  it('offers the file once ready, until it expires', async () => {
    const { page, text } = await open(exported('ready'));
    expect(text(page().querySelector('h2'))).toBe('my-tickets-2026-11-18.csv');
    expect(text(page().querySelector('.card p'))).toBe('Ready: 20001 rows.');
    expect(page().querySelector('a.download')?.getAttribute('href')).toBe('/api/v1/exports/e1/file');
    expect(text(page().querySelector('.note'))).toContain('by you only');
    expect(await axeViolations(page())).toEqual([]);
  });

  it('reads a queued export again until it is ready', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { page, text, reply } = await open(exported('queued'));
      expect(text(page().querySelector('[role="status"]'))).toBe('Being prepared: you will be notified when it is ready.');
      await vi.advanceTimersByTimeAsync(EXPORT_POLL_MS);
      await reply(exported('ready'));
      expect(page().querySelector('a.download')).not.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it('tells why it failed', async () => {
    const { page, text } = await open(exported('failed', { failure: 'query.invalid' }));
    expect(text(page().querySelector('[role="alert"]'))).toBe(
      'The query no longer runs (a field it names was deleted): export again from the list.',
    );
  });

  it('says when it is gone', async () => {
    const { page, text } = await open({ code: 'not_found', status: 404, title: 'Not found' }, 404);
    expect(text(page().querySelector('[role="status"]'))).toBe('This export has expired (files are kept 24 hours) or is not yours.');
  });
});
