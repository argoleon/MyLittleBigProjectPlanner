/**
 * Tests of rich text (L05-US01): the links the editor accepts, the editor
 * as a form control (value in and out, empty as null, the toolbar, links,
 * disabled), the server's HTML shown, and the priority label.
 */
import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { Router, provideRouter } from '@angular/router';

import { axeViolations } from '../testing/axe';
import { provideI18nTesting } from '../testing/i18n-testing';
import { Priority } from './priority';
import { RichText } from './rich-text';
import { RichTextDoc, RichTextEditor, Suggestion, isAllowedHref } from './rich-text-editor';

const settle = async () => {
  for (let i = 0; i < 3; i++) {
    await new Promise((resolve) => setTimeout(resolve));
    TestBed.tick();
  }
};

const DOC: RichTextDoc = {
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Replace the local accounts' }] }],
};

@Component({
  imports: [ReactiveFormsModule, RichTextEditor],
  template: `<ui-rich-text-editor [formControl]="control" label="Description" [invalid]="invalid()" />`,
})
class Host {
  readonly control = new FormControl<RichTextDoc | null>(DOC);
  readonly invalid = signal(false);
}

async function editor() {
  TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
  const fixture = TestBed.createComponent(Host);
  document.body.appendChild(fixture.nativeElement as HTMLElement);
  await settle();
  const root = fixture.nativeElement as HTMLElement;
  const component = fixture.debugElement.children[0].componentInstance as RichTextEditor;
  // The Tiptap editor itself, to select text as a user would.
  const tiptap = () => component['editor']!;
  const button = (label: string) => root.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
  const click = async (element: HTMLElement) => {
    element.click();
    await settle();
  };
  return { fixture, root, host: fixture.componentInstance, tiptap, button, click };
}

describe('isAllowedHref', () => {
  it('accepts web pages and email addresses only, as the server does', () => {
    expect(['https://example.org/a?b=1', 'http://intranet', 'mailto:team@example.org', ' HTTPS://x.org '].map(isAllowedHref)).toEqual([
      true,
      true,
      true,
      true,
    ]);
    expect(['javascript:alert(1)', 'data:text/html,x', 'ftp://x', '/relative', 'https://', 'https://a b'].map(isAllowedHref)).toEqual([
      false,
      false,
      false,
      false,
      false,
      false,
    ]);
  });
});

describe('RichTextEditor', () => {
  afterEach(() => document.body.replaceChildren());

  it('shows the value of its control in a labelled text box', async () => {
    const { root } = await editor();
    const box = root.querySelector('[role="textbox"]')!;
    expect(box.textContent).toBe('Replace the local accounts');
    expect(box.getAttribute('aria-multiline')).toBe('true');
    expect(document.getElementById(box.getAttribute('aria-labelledby')!)?.textContent).toBe('Description');
    expect(root.querySelector('[role="toolbar"]')?.getAttribute('aria-label')).toBe('Formatting');
    expect(await axeViolations(root)).toEqual([]);
  });

  it('formats the selection from the toolbar and gives the document to the form', async () => {
    const { host, tiptap, button, click } = await editor();
    tiptap().commands.selectAll();
    await click(button('Bold'));
    expect(button('Bold').getAttribute('aria-pressed')).toBe('true');
    expect(host.control.value).toEqual({
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Replace the local accounts', marks: [{ type: 'bold' }] }] }],
    });
    await click(button('Bulleted list'));
    expect(host.control.value?.content?.[0].type).toBe('bulletList');
    expect(button('Underline')).toBeNull();
  });

  it('gives null once emptied, and takes a value set by the form', async () => {
    const { host, root, tiptap } = await editor();
    tiptap().commands.clearContent(true);
    await settle();
    expect(host.control.value).toBeNull();
    host.control.setValue(DOC);
    await settle();
    expect(root.querySelector('[role="textbox"]')?.textContent).toBe('Replace the local accounts');
    expect(host.control.value).toEqual(DOC);
  });

  it('links the selection to web or email addresses only', async () => {
    const { root, host, tiptap, button, click } = await editor();
    tiptap().commands.selectAll();
    await click(button('Link'));
    expect(button('Link').getAttribute('aria-expanded')).toBe('true');
    const address = root.querySelector<HTMLInputElement>('input[type="url"]')!;
    expect(root.querySelector(`label[for="${address.id}"]`)?.textContent).toBe('Link address');
    address.value = 'javascript:alert(1)';
    await click(root.querySelector<HTMLButtonElement>('button.link-apply')!);
    expect(root.querySelector('[role="alert"]')?.textContent).toContain('Enter a web address');
    expect(address.getAttribute('aria-invalid')).toBe('true');
    expect(JSON.stringify(host.control.value)).not.toContain('link');

    address.value = 'https://example.org';
    await click(root.querySelector<HTMLButtonElement>('button.link-apply')!);
    const marks = host.control.value?.content?.[0].content?.[0].marks;
    expect(marks).toEqual([{ type: 'link', attrs: expect.objectContaining({ href: 'https://example.org' }) }]);
    expect(root.querySelector('input[type="url"]')).toBeNull();
    // Pressed again on a link: removed.
    await click(button('Link'));
    expect(host.control.value?.content?.[0].content?.[0].marks).toBeUndefined();
  });

  it('is read-only while its control is disabled', async () => {
    const { host, root, tiptap } = await editor();
    host.control.disable();
    await settle();
    expect(tiptap().isEditable).toBe(false);
    expect(Array.from(root.querySelectorAll('button')).every((b) => b.disabled)).toBe(true);
  });

  it('shows the error state the form asks for', async () => {
    const { fixture, host, root } = await editor();
    host.invalid.set(true);
    await fixture.whenStable();
    expect(root.querySelector('.frame')?.classList).toContain('invalid');
  });
});

const PEOPLE: Suggestion[] = [
  { id: 'a2', label: 'Camille Martin', detail: 'camille@acme.example' },
  { id: 'a3', label: 'Hugo Leroy', detail: 'hugo@acme.example' },
];
const TICKETS: Suggestion[] = [{ id: 'PLAT-120', label: 'PLAT-120', detail: 'Directory scoping' }];

@Component({
  imports: [ReactiveFormsModule, RichTextEditor],
  template: `<ui-rich-text-editor [formControl]="control" label="Comment" [mentions]="people" [issueRefs]="tickets" />`,
})
class SuggestingHost {
  readonly control = new FormControl<RichTextDoc | null>(null);
  readonly people = (query: string) => Promise.resolve(PEOPLE.filter((p) => p.label.toLowerCase().includes(query.toLowerCase())));
  readonly tickets = (query: string) => Promise.resolve(TICKETS.filter((t) => t.id.includes(query.toUpperCase())));
}

describe('RichTextEditor suggestions', () => {
  // jsdom lays nothing out: ProseMirror scrolls the caret into view with
  // these, which its Range lacks.
  beforeAll(() => {
    const empty = { length: 0, item: () => null, [Symbol.iterator]: [][Symbol.iterator] } as unknown as DOMRectList;
    Range.prototype.getClientRects ??= () => empty;
    Range.prototype.getBoundingClientRect ??= () => new DOMRect();
  });

  async function suggesting() {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(SuggestingHost);
    document.body.appendChild(fixture.nativeElement as HTMLElement);
    await settle();
    const root = fixture.nativeElement as HTMLElement;
    const component = fixture.debugElement.children[0].componentInstance as RichTextEditor;
    const tiptap = () => component['editor']!;
    const type = async (text: string) => {
      tiptap().chain().focus().insertContent(text).run();
      await settle();
    };
    const key = async (name: string) => {
      tiptap().view.dom.dispatchEvent(new KeyboardEvent('keydown', { key: name, bubbles: true, cancelable: true }));
      await settle();
    };
    const options = () =>
      Array.from(root.querySelectorAll('[role="option"]')).map((o) => (o.querySelector('.suggestion-label') ?? o).textContent?.trim());
    return { root, host: fixture.componentInstance, tiptap, type, key, options };
  }

  afterEach(() => document.body.replaceChildren());

  it('offers people after @, driven by the keyboard, the focus staying in the text', async () => {
    const { root, host, tiptap, type, key, options } = await suggesting();
    await type('Thanks @');
    expect(options()).toEqual(['Camille Martin', 'Hugo Leroy']);
    expect(root.querySelector('.suggestion-detail')?.textContent).toBe('camille@acme.example');
    const box = tiptap().view.dom;
    const list = root.querySelector('[role="listbox"]')!;
    expect(box.getAttribute('aria-autocomplete')).toBe('list');
    expect(box.getAttribute('aria-controls')).toBe(list.id);
    expect(box.getAttribute('aria-activedescendant')).toBe(list.querySelectorAll('[role="option"]')[0].id);
    expect(await axeViolations(root)).toEqual([]);
    await key('ArrowDown');
    expect(list.querySelectorAll('[role="option"]')[1].getAttribute('aria-selected')).toBe('true');
    expect(box.getAttribute('aria-activedescendant')).toBe(list.querySelectorAll('[role="option"]')[1].id);
    await key('Enter');
    expect(root.querySelector('[role="listbox"]')).toBeNull();
    expect(box.hasAttribute('aria-activedescendant')).toBe(false);
    const inserted = host.control.value?.content?.[0].content?.[1];
    expect(inserted).toEqual(expect.objectContaining({ type: 'mention', attrs: expect.objectContaining({ id: 'a3', label: 'Hugo Leroy' }) }));
    expect(root.querySelector('.mention')?.textContent).toBe('@Hugo Leroy');
  });

  it('narrows the people as one types, says when none matches, and closes with Escape', async () => {
    const { root, type, key, options } = await suggesting();
    await type('@cam');
    expect(options()).toEqual(['Camille Martin']);
    await type('zz');
    expect(options()).toEqual(['No match.']);
    await key('Escape');
    expect(root.querySelector('[role="listbox"]')).toBeNull();
  });

  it('offers tickets after #, picked with the mouse, shown by their key', async () => {
    const { root, host, type } = await suggesting();
    await type('See #pla');
    const option = root.querySelector('[role="option"]')!;
    option.dispatchEvent(new MouseEvent('mousedown', { bubbles: true, cancelable: true }));
    await settle();
    const inserted = host.control.value?.content?.[0].content?.[1];
    expect(inserted).toEqual(expect.objectContaining({ type: 'issueRef', attrs: expect.objectContaining({ id: 'PLAT-120', label: 'PLAT-120' }) }));
    expect(root.querySelector('.issue-ref')?.textContent).toBe('PLAT-120');
  });
});

describe('RichText', () => {
  it('shows the HTML the server rendered', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(RichText);
    fixture.componentRef.setInput('html', '<p>Replace <strong>local</strong> &lt;now&gt;</p><ul><li><p>Entra ID</p></li></ul>');
    await fixture.whenStable();
    const root = fixture.nativeElement as HTMLElement;
    expect(root.querySelector('strong')?.textContent).toBe('local');
    expect(root.querySelector('p')?.textContent).toBe('Replace local <now>');
    expect(root.querySelectorAll('li')).toHaveLength(1);
  });

  it('opens a ticket it links in the application, a modified click left to the browser', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting(), provideRouter([])] });
    const router = TestBed.inject(Router);
    const navigate = vi.spyOn(router, 'navigateByUrl').mockResolvedValue(true);
    const fixture = TestBed.createComponent(RichText);
    fixture.componentRef.setInput(
      'html',
      '<p>See <a class="issue-ref" href="/issues/PLAT-120">PLAT-120</a> and <a href="https://example.org">docs</a></p>',
    );
    await fixture.whenStable();
    const root = fixture.nativeElement as HTMLElement;
    const click = (link: Element, init: MouseEventInit = {}) => {
      const event = new MouseEvent('click', { bubbles: true, cancelable: true, ...init });
      link.dispatchEvent(event);
      return event.defaultPrevented;
    };
    expect(click(root.querySelector('a.issue-ref')!)).toBe(true);
    expect(navigate).toHaveBeenCalledWith('/issues/PLAT-120');
    expect(click(root.querySelector('a.issue-ref')!, { ctrlKey: true })).toBe(false);
    expect(click(root.querySelector('a:not(.issue-ref)')!)).toBe(false);
    expect(navigate).toHaveBeenCalledTimes(1);
  });
});

describe('Priority', () => {
  it('shows the priority by its name, the icon hidden from screen readers', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(Priority);
    fixture.componentRef.setInput('priority', 'highest');
    await fixture.whenStable();
    const root = fixture.nativeElement as HTMLElement;
    expect(root.textContent?.replace('keyboard_double_arrow_up', '').trim()).toBe('Highest');
    expect(root.querySelector('mat-icon')?.getAttribute('aria-hidden')).toBe('true');
  });
});
