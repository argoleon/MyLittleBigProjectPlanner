/**
 * Tests of the visual query builder (L06-US02), the API mocked at the HTTP
 * level: the lists of types and statuses with their labels, criteria chosen
 * from selects and chips, presets and custom ranges of dates, the text,
 * and accessibility.
 */
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Suggestion, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { CURRENT_USER, Criteria, NO_CRITERIA } from './criteria';
import { QueryBuilder } from './query-builder';

const value = (text: string, label: string, detail: string | null = null): Suggestion => ({ kind: 'value', text, label, detail });

/** What the API suggests after `<field> IN (<typed>`. */
const LISTS: Record<string, Suggestion[]> = {
  'type IN (': [value('bug', 'Bug'), value('story', 'Story')],
  'status IN (': [value('todo', 'To do', 'todo'), value('in_progress', 'In progress', 'in_progress')],
  'project IN (pl': [value('PLAT', 'Identity platform', 'project')],
  'assignee IN (cam': [value('camille@acme.example', 'Camille Martin')],
};

@Component({
  imports: [QueryBuilder],
  template: `<app-query-builder [(criteria)]="criteria" />`,
})
class Host {
  readonly criteria = signal<Criteria>(NO_CRITERIA);
}

describe('the query builder', () => {
  async function setUp() {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const fixture = TestBed.createComponent(Host);
    const http = TestBed.inject(HttpTestingController);
    const settle = async (ms = 20) => {
      for (let i = 0; i < 3; i++) {
        await new Promise((resolve) => setTimeout(resolve, ms));
        for (const asked of http.match((r) => r.url === '/api/v1/search/suggestions')) {
          const query = (asked.request.body as { query: string }).query;
          asked.flush({ from: 0, to: 0, suggestions: LISTS[query] ?? [] });
        }
        fixture.detectChanges();
        await fixture.whenStable();
      }
    };
    await settle();
    const host = fixture.nativeElement as HTMLElement;
    const click = async (element: Element | null | undefined) => {
      (element as HTMLElement).click();
      await settle();
    };
    const option = (text: string) =>
      Array.from(document.querySelectorAll('mat-option')).find((o) => o.textContent?.trim() === text) as HTMLElement;
    const close = async () => {
      document.querySelector('.cdk-overlay-backdrop')?.dispatchEvent(new MouseEvent('click'));
      await settle();
    };
    const typeIn = async (input: HTMLInputElement, text: string, ms = 20) => {
      input.focus();
      input.value = text;
      input.dispatchEvent(new Event('input'));
      await settle(ms);
    };
    return { fixture, host, settle, click, option, close, typeIn };
  }

  it('offers the types and statuses with their labels and writes the chosen keys', async () => {
    const { fixture, host, click, option, close } = await setUp();
    await click(host.querySelector('mat-select.statuses'));
    expect(Array.from(document.querySelectorAll('mat-option')).map((o) => o.textContent?.trim())).toEqual(['To do', 'In progress']);
    await click(option('In progress'));
    await close();
    await click(host.querySelector('mat-select.priorities'));
    await click(option('High'));
    await close();
    expect(fixture.componentInstance.criteria()).toEqual({ ...NO_CRITERIA, statuses: ['in_progress'], priorities: ['high'] });
  });

  it('adds projects and people from what is typed, and me', async () => {
    const { fixture, host, click, option, typeIn } = await setUp();
    await typeIn(host.querySelector('input.projects') as HTMLInputElement, 'pl');
    await click(option('PLAT · Identity platform'));
    expect(Array.from(host.querySelectorAll('mat-chip-row')).map((c) => c.textContent?.replace('cancel', '').trim())).toEqual([
      'Identity platform',
    ]);
    const people = host.querySelector('input.assignees') as HTMLInputElement;
    await typeIn(people, 'cam');
    await click(option('Camille Martin · camille@acme.example'));
    await typeIn(people, '');
    await click(option('Me'));
    expect(fixture.componentInstance.criteria()).toMatchObject({
      projects: ['PLAT'],
      assignees: ['camille@acme.example', CURRENT_USER],
    });
    await click(host.querySelector('button[aria-label="Remove Identity platform"]'));
    expect(fixture.componentInstance.criteria().projects).toEqual([]);
  });

  it('chooses a date preset, or a custom range shown from the criteria', async () => {
    const { fixture, host, click, option, settle } = await setUp();
    await click(host.querySelector('mat-select.due'));
    await click(option('Overdue'));
    expect(fixture.componentInstance.criteria().due).toBe('overdue');
    expect(host.querySelector('input.due-from')).toBeNull();

    fixture.componentInstance.criteria.set({ ...NO_CRITERIA, due: { from: '2026-11-01', to: '2026-11-30' }, created: 'last_7_days' });
    await settle();
    expect(host.querySelector('mat-select.due')?.textContent?.trim()).toBe('Custom range…');
    expect((host.querySelector('input.due-from') as HTMLInputElement).value).toBe('11/1/2026');
    expect(host.querySelector('mat-select.created')?.textContent?.trim()).toBe('Last 7 days');
    expect(await axeViolations(host)).toEqual([]);
  });

  it('writes the text after a pause in typing', async () => {
    const { fixture, host, typeIn } = await setUp();
    await typeIn(host.querySelector('input.text') as HTMLInputElement, 'outage', 450);
    expect(fixture.componentInstance.criteria().text).toBe('outage');
  });
});
