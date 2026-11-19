/**
 * Tests of the page of a bulk action (L06-US08), where its notification
 * leads: its report once done, read again while it runs, gone once
 * expired, and accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { provideRouter, withComponentInputBinding } from '@angular/router';
import { RouterTestingHarness } from '@angular/router/testing';
import { BulkAction, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { BULK_POLL_MS, BulkActionPage } from './bulk-action-page';

function bulk(status: BulkAction['status']): BulkAction {
  const done = status === 'done';
  return {
    id: 'b1',
    kind: 'transition',
    status,
    background: true,
    total: 2,
    counts: { changed: done ? 1 : 0, unchanged: 0, refused: done ? 1 : 0, withParent: 0, waiting: done ? 0 : 2 },
    items: [
      { issueId: 'i1', key: 'PLAT-1', outcome: done ? 'changed' : null, errors: [] },
      { issueId: 'i2', key: 'PLAT-2', outcome: done ? 'refused' : null, reason: done ? 'permission.denied' : null, errors: [] },
    ],
    createdAt: '2026-11-19T09:00:00Z',
    completedAt: done ? '2026-11-19T09:01:00Z' : null,
    expiresAt: '2026-11-26T09:00:00Z',
  };
}

describe('the page of a bulk action', () => {
  async function open(first: object, status = 200) {
    TestBed.configureTestingModule({
      providers: [
        provideRouter([{ path: 'bulk-actions/:id', component: BulkActionPage }], withComponentInputBinding()),
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
        [request] = http.match((r) => r.url === '/api/v1/bulk-actions/b1');
        if (!request) await new Promise((resolve) => setTimeout(resolve, 20));
      }
      if (!request) throw new Error('no bulk action read');
      request.flush(body, { status: code, statusText: 'S', headers: code >= 400 ? { 'content-type': 'application/problem+json' } : {} });
      await settle();
      return request;
    };
    const harness = await RouterTestingHarness.create();
    await harness.navigateByUrl('/bulk-actions/b1');
    await reply(first, status);
    const page = () => harness.routeNativeElement as HTMLElement;
    const text = (element: Element | null | undefined) => element?.textContent?.replace(/\s+/g, ' ').trim();
    return { page, text, reply, http };
  }

  it('shows the report of a bulk action done', async () => {
    const { page, text } = await open(bulk('done'));
    expect(text(page().querySelector('h1'))).toBe('Bulk status change');
    expect(text(page().querySelector('.counts'))).toBe('1 changed, 0 already so, 1 left unchanged');
    expect(page().querySelectorAll('tbody tr').length).toBe(2);
    expect(await axeViolations(page())).toEqual([]);
  });

  it('reads a running bulk action again until it is done', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const { page, text, reply } = await open(bulk('running'));
      expect(text(page().querySelector('.note'))).toBe('Still running: this page reads it again.');
      expect(text(page().querySelector('.counts'))).toBe('0 changed, 0 already so, 0 left unchanged · 2 waiting');
      await vi.advanceTimersByTimeAsync(BULK_POLL_MS);
      await reply(bulk('done'));
      expect(page().querySelector('.note')).toBeNull();
      expect(text(page().querySelector('.counts'))).toBe('1 changed, 0 already so, 1 left unchanged');
    } finally {
      vi.useRealTimers();
    }
  });

  it('says when it is gone', async () => {
    const { page, text } = await open({ code: 'not_found', status: 404, title: 'Not found' }, 404);
    expect(text(page().querySelector('[role="status"]'))).toBe('This report has expired (reports are kept 7 days) or is not yours.');
  });
});
