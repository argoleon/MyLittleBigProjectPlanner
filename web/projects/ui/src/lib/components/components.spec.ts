import { Component, Type, signal } from '@angular/core';
import { ComponentFixture, TestBed } from '@angular/core/testing';

import { ApiProblem } from '../api/api-problem';
import { LocaleService } from '../i18n/locale.service';
import { ThemeService } from '../theming/theme.service';
import { axeViolations } from '../testing/axe';
import { provideI18nTesting } from '../testing/i18n-testing';
import { EmptyState } from './empty-state';
import { ErrorState } from './error-state';
import { LoadingState } from './loading-state';
import { AccountTypeChip, MemberStatusChip } from './member-chips';
import { StatusChip } from './status-chip';
import { TypeChip } from './type-chip';

async function render<T>(component: Type<T>, inputs: Record<string, unknown> = {}) {
  TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
  const fixture = TestBed.createComponent(component);
  for (const [name, value] of Object.entries(inputs)) {
    fixture.componentRef.setInput(name, value);
  }
  await fixture.whenStable();
  return fixture;
}

const text = (fixture: ComponentFixture<unknown>) =>
  ((fixture.nativeElement as HTMLElement).textContent ?? '').replace(/\s+/g, ' ').trim();

beforeEach(() => localStorage.clear());

describe('StatusChip', () => {
  it('names the category by default, prefixed for screen readers', async () => {
    const fixture = await render(StatusChip, { category: 'progress' });
    expect(text(fixture)).toBe('Status: In progress');
    expect((fixture.nativeElement as HTMLElement).querySelector('.cdk-visually-hidden')?.textContent?.trim()).toBe('Status:');
    expect((fixture.nativeElement as HTMLElement).querySelector('.chip')?.getAttribute('data-category')).toBe('progress');
  });

  it('shows the configured status name', async () => {
    const fixture = await render(StatusChip, { category: 'done', label: 'Shipped' });
    expect(text(fixture)).toBe('Status: Shipped');
  });
});

describe('TypeChip', () => {
  it('shows the icon, hidden from screen readers, and the type name', async () => {
    const fixture = await render(TypeChip, { kind: 'bug' });
    const element = fixture.nativeElement as HTMLElement;
    expect(element.querySelector('mat-icon')?.getAttribute('aria-hidden')).toBe('true');
    expect(element.querySelector('mat-icon')?.textContent).toBe('bug_report');
    expect(element.querySelector('.chip')?.getAttribute('data-kind')).toBe('bug');
    expect(element.querySelector('.chip')?.textContent?.replace('bug_report', '').trim()).toBe('Type: Bug');
  });

  it('shows the configured type name', async () => {
    const fixture = await render(TypeChip, { kind: 'story', label: 'User story' });
    expect(text(fixture)).toContain('Type: User story');
  });
});

describe('AccountTypeChip and MemberStatusChip', () => {
  it('name the type and status, prefixed for screen readers', async () => {
    const type = await render(AccountTypeChip, { type: 'external' });
    expect(text(type)).toBe('Account type: External');
    expect((type.nativeElement as HTMLElement).querySelector('.chip')?.getAttribute('data-kind')).toBe('external');
    TestBed.resetTestingModule();
    const status = await render(MemberStatusChip, { state: 'invited' });
    expect(text(status)).toBe('Status: Invitation sent');
    expect((status.nativeElement as HTMLElement).querySelector('.chip')?.getAttribute('data-state')).toBe('invited');
    expect(await axeViolations(status.nativeElement)).toEqual([]);
  });
});

describe('EmptyState', () => {
  it('shows a heading at the requested level, a message and the projected actions', async () => {
    @Component({
      imports: [EmptyState],
      template: `<ui-empty-state title="No ticket yet" message="Create the first one." [headingLevel]="3"><button>Create</button></ui-empty-state>`,
    })
    class Host {}
    const fixture = await render(Host);
    const element = fixture.nativeElement as HTMLElement;
    const heading = element.querySelector('[role="heading"]');
    expect(heading?.textContent).toBe('No ticket yet');
    expect(heading?.getAttribute('aria-level')).toBe('3');
    expect(element.querySelector('.message')?.textContent).toBe('Create the first one.');
    expect(element.querySelector('.actions button')?.textContent).toBe('Create');
  });

  it('omits the message when there is none', async () => {
    const fixture = await render(EmptyState, { title: 'Nothing' });
    expect((fixture.nativeElement as HTMLElement).querySelector('.message')).toBeNull();
  });
});

describe('ErrorState', () => {
  it('is an alert with the translated message of an API problem', async () => {
    const fixture = await render(ErrorState, { error: new ApiProblem(412, 'precondition_failed', 'Modified') });
    const element = fixture.nativeElement as HTMLElement;
    expect(element.querySelector('[role="alert"]')).not.toBeNull();
    expect(element.querySelector('[role="heading"]')?.textContent).toBe('Something went wrong');
    expect(element.querySelector('.message')?.textContent).toBe(
      'Someone changed this item in the meantime. Reload it before trying again.',
    );
  });

  it('shows a generic message for other errors', async () => {
    const fixture = await render(ErrorState, { error: new TypeError('boom') });
    expect((fixture.nativeElement as HTMLElement).querySelector('.message')?.textContent).toBe(
      'An unexpected error occurred.',
    );
  });

  it('emits retry from its button, which can be hidden', async () => {
    const fixture = await render(ErrorState, { error: null });
    let retries = 0;
    fixture.componentInstance.retry.subscribe(() => retries++);
    const button = (fixture.nativeElement as HTMLElement).querySelector('button');
    expect(button?.textContent?.trim()).toBe('Try again');
    button?.click();
    expect(retries).toBe(1);

    fixture.componentRef.setInput('retryable', false);
    await fixture.whenStable();
    expect((fixture.nativeElement as HTMLElement).querySelector('button')).toBeNull();
  });
});

describe('LoadingState', () => {
  it('is a polite status with an accessible progress bar', async () => {
    const fixture = await render(LoadingState);
    const element = fixture.nativeElement as HTMLElement;
    expect(element.querySelector('[role="status"]')?.textContent?.trim()).toBe('Loading…');
    expect(element.querySelector('mat-progress-bar')?.getAttribute('aria-label')).toBe('Loading…');
  });

  it('announces a custom text', async () => {
    const fixture = await render(LoadingState, { label: 'Loading tickets…' });
    expect(text(fixture)).toBe('Loading tickets…');
  });
});

@Component({
  imports: [StatusChip, TypeChip, EmptyState, ErrorState, LoadingState],
  template: `
    <ui-status-chip category="todo" />
    <ui-status-chip category="progress" />
    <ui-status-chip category="done" />
    <ui-type-chip kind="story" />
    <ui-type-chip kind="bug" />
    <ui-type-chip kind="task" />
    <ui-type-chip kind="epic" />
    <ui-empty-state title="Nothing yet" message="Create one." />
    <ui-error-state [error]="error()" />
    <ui-loading-state />
  `,
})
class Gallery {
  readonly error = signal(new ApiProblem(0, 'network', 'offline'));
}

describe('shared components together', () => {
  for (const mode of ['light', 'dark'] as const) {
    for (const language of ['fr', 'en'] as const) {
      it(`render in ${mode} mode, in ${language}, without accessibility violation`, async () => {
        TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
        TestBed.inject(ThemeService).use(mode);
        TestBed.inject(LocaleService).use(language);
        const fixture = TestBed.createComponent(Gallery);
        await fixture.whenStable();

        expect(document.documentElement.dataset['theme']).toBe(mode);
        const element = fixture.nativeElement as HTMLElement;
        expect(element.querySelectorAll('.chip')).toHaveLength(7);
        // Colors only come from the theme tokens: no inline style.
        expect(element.querySelectorAll('[style*="color"]')).toHaveLength(0);
        expect(element.textContent).toContain(language === 'fr' ? 'Le serveur est injoignable' : 'The server cannot be reached');
        expect(await axeViolations(element)).toEqual([]);
      });
    }
  }
});
