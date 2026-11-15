/**
 * Tests of watching a ticket (L05-US07), the API mocked at the HTTP level
 * and the real translations: the state and count, a toggle to watch and
 * to stop, a refusal said; accessibility.
 */
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { TestBed } from '@angular/core/testing';
import { Issue, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { IssueWatch } from './issue-watch';

const ISSUE = { id: 'i142', key: 'PLAT-142', version: 4 } as Issue;

describe('watching a ticket', () => {
  async function open() {
    localStorage.clear();
    TestBed.configureTestingModule({
      providers: [provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const http = TestBed.inject(HttpTestingController);
    const fixture = TestBed.createComponent(IssueWatch);
    fixture.componentRef.setInput('issue', ISSUE);
    const settle = async () => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve));
        fixture.detectChanges();
      }
    };
    const reply = async (method: string, body: object, status = 200): Promise<TestRequest> => {
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        fixture.detectChanges();
        [request] = http.match((r) => r.method === method && r.url.startsWith('/api/v1/issues/i142/watch'));
        if (!request) await new Promise((resolve) => setTimeout(resolve, 10));
      }
      if (!request) throw new Error(`no ${method}`);
      request.flush(body, { status, statusText: status < 400 ? 'OK' : 'Error' });
      await settle();
      return request;
    };
    const root = fixture.nativeElement as HTMLElement;
    const button = () => root.querySelector('button.watch') as HTMLButtonElement;
    const text = () => button().textContent?.replace('visibility_off', '').replace('visibility', '').trim();
    return { root, reply, settle, button, text };
  }

  it('watches and stops watching', async () => {
    const { root, reply, settle, button, text } = await open();
    await reply('GET', { count: 1, watching: false });
    expect(text()).toBe('Watch');
    expect(button().getAttribute('aria-pressed')).toBe('false');
    button().click();
    await settle();
    await reply('PUT', { count: 2, watching: true });
    expect(text()).toBe('Watching (2)');
    expect(button().getAttribute('aria-pressed')).toBe('true');
    expect(await axeViolations(root)).toEqual([]);
    button().click();
    await settle();
    await reply('DELETE', { count: 1, watching: false });
    expect(text()).toBe('Watch');
  });

  it('says why it could not', async () => {
    const { root, reply, settle, button } = await open();
    await reply('GET', { count: 1, watching: false });
    button().click();
    await settle();
    await reply('PUT', { type: 'about:blank', title: 'Not found', status: 404, code: 'not_found' }, 404);
    expect(root.querySelector('[role="alert"]')?.textContent?.trim()).toBeTruthy();
    expect(button().disabled).toBe(false);
  });
});
