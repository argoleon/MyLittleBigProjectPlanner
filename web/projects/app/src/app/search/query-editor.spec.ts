/**
 * Tests of the query editor (L06-US02), the API mocked at the HTTP level:
 * where a chosen suggestion goes, the suggestions following the cursor,
 * the keyboard alone (Ctrl+Space, arrows, Enter to accept, Enter to run),
 * a refused query marked on the field, and accessibility.
 */
import { provideHttpClient } from '@angular/common/http';
import { HttpTestingController, TestRequest, provideHttpClientTesting } from '@angular/common/http/testing';
import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { Suggestion, provideApi, provideMaterialDefaults } from 'ui';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { QueryEditor, optionOf } from './query-editor';

const field = (text: string): Suggestion => ({ kind: 'field', text, label: null, detail: null });
const value = (text: string, label: string): Suggestion => ({ kind: 'value', text, label, detail: 'in_progress' });

describe('where a chosen suggestion goes', () => {
  it('replaces the word typed, then a space and the cursor after it', () => {
    expect(optionOf('stat', 0, 4, field('status'))).toMatchObject({ query: 'status ', caret: 7 });
    expect(optionOf('status = in AND x', 9, 11, value('in_progress', 'In progress'))).toMatchObject({
      query: 'status = in_progress AND x',
      caret: 21,
    });
    expect(optionOf('status = ', 9, 9, value('todo', 'To do'))).toMatchObject({ query: 'status = todo ', caret: 14 });
  });

  it('keeps the cursor inside a function that takes an argument', () => {
    const descendants: Suggestion = { kind: 'function', text: 'descendantsOf("")', label: null, detail: null, caret: 15 };
    expect(optionOf('project IN ', 11, 11, descendants)).toMatchObject({
      query: 'project IN descendantsOf("")',
      caret: 26,
    });
  });
});

@Component({
  imports: [QueryEditor],
  template: `<app-query-editor [(value)]="query" [describedBy]="describedBy()" [invalid]="invalid()" (submitted)="runs.set(runs() + 1)" />`,
})
class Host {
  readonly query = signal('');
  readonly describedBy = signal<string | null>(null);
  readonly invalid = signal(false);
  readonly runs = signal(0);
}

describe('the query editor', () => {
  async function setUp() {
    TestBed.configureTestingModule({
      providers: [provideHttpClient(), provideApi(), provideHttpClientTesting(), provideI18nTesting(), provideMaterialDefaults()],
    });
    const fixture = TestBed.createComponent(Host);
    const http = TestBed.inject(HttpTestingController);
    const settle = async (ms = 20) => {
      await new Promise((resolve) => setTimeout(resolve, ms));
      fixture.detectChanges();
      await fixture.whenStable();
    };
    await settle();
    const textarea = fixture.nativeElement.querySelector('textarea') as HTMLTextAreaElement;
    const type = async (text: string) => {
      textarea.focus();
      textarea.value = text;
      textarea.setSelectionRange(text.length, text.length);
      textarea.dispatchEvent(new Event('input'));
      await settle(200);
    };
    const suggested = async (from: number, to: number, suggestions: Suggestion[]): Promise<TestRequest> => {
      // Asked after a short wait: 1 s at most.
      let request: TestRequest | undefined;
      for (let i = 0; i < 50 && !request; i++) {
        [request] = http.match((r) => r.url === '/api/v1/search/suggestions');
        if (!request) await settle();
      }
      if (!request) throw new Error('no suggestion request');
      request.flush({ from, to, suggestions });
      await settle();
      return request;
    };
    // The autocomplete reads keyCode, as browsers set it.
    const codes: Record<string, number> = { Enter: 13, Escape: 27, ' ': 32, ArrowDown: 40 };
    const key = async (key: string, init: KeyboardEventInit = {}) => {
      const event = new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
      Object.defineProperty(event, 'keyCode', { value: codes[key] ?? 0 });
      textarea.dispatchEvent(event);
      await settle();
    };
    /** Waits until `check` holds (the panel opens after several async steps), 2 s at most. */
    const until = async (check: () => boolean) => {
      for (let i = 0; i < 100 && !check(); i++) await settle();
    };
    const expanded = () => textarea.getAttribute('aria-expanded') === 'true';
    const options = () => Array.from(document.querySelectorAll('mat-option')).map((o) => o.textContent?.replace(/\s+/g, ' ').trim());
    return { fixture, http, settle, textarea, type, suggested, key, options, until, expanded };
  }

  it('suggests what follows the cursor, the word typed replaced by the choice', async () => {
    const { fixture, textarea, type, suggested, key, options, until, expanded } = await setUp();
    await type('stat');
    const asked = await suggested(0, 4, [field('status')]);
    expect(asked.request.body).toEqual({ query: 'stat', cursor: 4 });
    expect(textarea.getAttribute('role')).toBe('combobox');
    await until(expanded);
    expect(textarea.getAttribute('aria-expanded')).toBe('true');
    expect(options()).toEqual(['status, Status, field']);

    await key('ArrowDown');
    expect(textarea.getAttribute('aria-activedescendant')).toBeTruthy();
    await key('Enter');
    expect(textarea.value).toBe('status ');
    expect(fixture.componentInstance.query()).toBe('status ');
    expect(textarea.selectionStart).toBe(7);
    expect(fixture.componentInstance.runs(), 'Enter accepted the suggestion').toBe(0);

    // On to what may follow the field: its values, with their labels.
    const next = await suggested(9, 9, [value('in_progress', 'In progress')]);
    expect(next.request.body).toEqual({ query: 'status ', cursor: 7 });
    await until(() => options().length > 0);
    expect(options()).toEqual(['in_progress, In progress, value']);
    expect(await axeViolations(document.body)).toEqual([]);
  });

  it('opens on Ctrl+Space, closes on Escape, and runs the query on Enter otherwise', async () => {
    const { fixture, textarea, type, suggested, key, settle, until, expanded } = await setUp();
    await type('status = todo');
    await suggested(14, 14, []);
    expect(textarea.getAttribute('aria-expanded')).toBe('false');
    await key(' ', { ctrlKey: true });
    await settle();
    await suggested(14, 14, [{ kind: 'keyword', text: 'AND', label: null, detail: null }]);
    await until(expanded);
    expect(textarea.getAttribute('aria-expanded')).toBe('true');
    await key('Escape');
    expect(textarea.getAttribute('aria-expanded')).toBe('false');
    await key('Enter');
    expect(fixture.componentInstance.runs()).toBe(1);
    await key('Enter', { shiftKey: true });
    expect(fixture.componentInstance.runs(), 'Shift+Enter starts a new line').toBe(1);
  });

  it('takes the query given, and marks a refused one', async () => {
    const { fixture, textarea, settle, http } = await setUp();
    fixture.componentInstance.query.set('status = ');
    fixture.componentInstance.describedBy.set('query-error');
    fixture.componentInstance.invalid.set(true);
    await settle();
    expect(textarea.value).toBe('status = ');
    expect(textarea.getAttribute('aria-describedby')).toBe('query-error');
    expect(textarea.getAttribute('aria-invalid')).toBe('true');
    expect(http.match(() => true), 'a value given from outside asks for nothing').toEqual([]);
  });
});
