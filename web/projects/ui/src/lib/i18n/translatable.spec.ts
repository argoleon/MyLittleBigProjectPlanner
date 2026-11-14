import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { FormGroup, ReactiveFormsModule } from '@angular/forms';

import { TranslatableInput } from '../components/translatable-input';
import { axeViolations } from '../testing/axe';
import { provideI18nTesting } from '../testing/i18n-testing';
import { LocaleService } from './locale.service';
import {
  TranslatedPipe,
  resolveLabel,
  translationLanguage,
  translationsForm,
  translationsValue,
} from './translatable';

describe('resolveLabel', () => {
  it('prefers the language, then the fallback, then any', () => {
    const both = { fr: 'Développement logiciel', en: 'Software development' };
    expect(resolveLabel(both, 'en')).toBe('Software development');
    expect(resolveLabel(both, 'fr')).toBe('Développement logiciel');
    expect(resolveLabel({ fr: 'Développement' }, 'en')).toBe('Développement');
    expect(resolveLabel({ en: 'Software' }, 'fr', 'fr')).toBe('Software');
    expect(resolveLabel({ fr: 'Développement', en: 'Software' }, 'en', 'fr')).toBe('Software');
  });

  it('skips blank texts and gives nothing when there is none', () => {
    expect(resolveLabel({ en: '  ', fr: 'Support' }, 'en')).toBe('Support');
    expect(resolveLabel({ en: ' Help desk ' }, 'en')).toBe('Help desk');
    expect(resolveLabel({}, 'en')).toBe('');
    expect(resolveLabel(null, 'fr')).toBe('');
  });
});

describe('translationsForm', () => {
  it('requires the fallback language only', () => {
    const form = translationsForm();
    expect(form.controls.fr.hasError('required')).toBe(true);
    expect(form.controls.en.valid).toBe(true);
    form.controls.fr.setValue('   ');
    expect(form.controls.fr.hasError('required')).toBe(true);
    form.controls.fr.setValue('Support');
    expect(form.valid).toBe(true);

    const english = translationsForm({}, { fallback: 'en' });
    expect(english.controls.en.hasError('required')).toBe(true);
    expect(english.controls.fr.valid).toBe(true);
  });

  it('requires nothing for an optional label', () => {
    const form = translationsForm({}, { required: false });
    expect(form.valid).toBe(true);
    form.controls.en.setValue('x'.repeat(101));
    expect(form.controls.en.hasError('too_long')).toBe(true);
  });

  it('limits every language once trimmed', () => {
    const form = translationsForm({ fr: 'Support' }, { maxLength: 5 });
    form.controls.en.setValue(' Desks ');
    expect(form.controls.en.valid).toBe(true);
    form.controls.en.setValue('Help desk');
    expect(form.controls.en.hasError('too_long')).toBe(true);
    expect(form.controls.fr.hasError('too_long')).toBe(true);
  });

  it('starts from a label and gives the API texts trimmed, blank ones left out', () => {
    const form = translationsForm({ fr: 'Développement', en: 'Software' });
    expect(form.getRawValue()).toEqual({ fr: 'Développement', en: 'Software' });
    form.setValue({ fr: '  Développement logiciel ', en: ' ' });
    expect(translationsValue(form)).toEqual({ fr: 'Développement logiciel' });
  });

  it('tells which language an API error names', () => {
    expect(translationLanguage('nameTranslations.en', 'nameTranslations')).toBe('en');
    expect(translationLanguage('nameTranslations.de', 'nameTranslations')).toBeNull();
    expect(translationLanguage('nameTranslations', 'nameTranslations')).toBeNull();
    expect(translationLanguage('labelTranslations.fr', 'nameTranslations')).toBeNull();
  });
});

describe('TranslatedPipe', () => {
  it('follows the language of the user', async () => {
    @Component({ imports: [TranslatedPipe], template: `<span>{{ name() | translated }}</span>` })
    class Host {
      readonly name = signal({ fr: 'Développement logiciel', en: 'Software development' });
    }
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    const span = () => (fixture.nativeElement as HTMLElement).querySelector('span')?.textContent;
    expect(span()).toBe('Software development');
    TestBed.inject(LocaleService).use('fr');
    await fixture.whenStable();
    expect(span()).toBe('Développement logiciel');
    fixture.componentInstance.name.set({ fr: 'Support', en: '' });
    TestBed.inject(LocaleService).use('en');
    await fixture.whenStable();
    expect(span()).toBe('Support');
  });
});

describe('TranslatableInput', () => {
  @Component({
    imports: [ReactiveFormsModule, TranslatableInput],
    template: `
      <form [formGroup]="form">
        <ui-translatable-input [group]="form.controls.name" label="Name" />
      </form>
    `,
  })
  class Host {
    readonly form = new FormGroup({ name: translationsForm({ en: 'Software' }) });
  }

  async function render() {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    const element = fixture.nativeElement as HTMLElement;
    return { fixture, element, form: fixture.componentInstance.form };
  }

  it('edits each language side by side, each field labelled with its language', async () => {
    const { element, fixture, form } = await render();
    const inputs = Array.from(element.querySelectorAll('input'));
    expect(inputs.map((input) => input.getAttribute('lang'))).toEqual(['fr', 'en']);
    const labels = Array.from(element.querySelectorAll('mat-label')).map((label) => label.textContent?.trim());
    expect(labels).toEqual(['Name (Français)', 'Name (English)']);
    expect(inputs[0].required).toBe(true);
    expect(inputs[1].required).toBe(false);
    expect(inputs[1].value).toBe('Software');
    const hints = Array.from(element.querySelectorAll('mat-hint')).map((hint) => hint.textContent?.trim());
    expect(hints).toEqual(['Fallback language: required', 'Optional']);

    // Typed in, it fills the form.
    inputs[0].value = 'Développement';
    inputs[0].dispatchEvent(new Event('input'));
    await fixture.whenStable();
    expect(form.value.name).toEqual({ fr: 'Développement', en: 'Software' });
    expect(form.valid).toBe(true);
    expect(await axeViolations(element)).toEqual([]);
  });

  it('says which language is missing once the form is touched, and shows API errors', async () => {
    const { element, fixture, form } = await render();
    expect(element.querySelector('mat-error')).toBeNull();
    form.markAllAsTouched();
    fixture.detectChanges();
    await fixture.whenStable();
    expect(element.querySelector('mat-error')?.textContent?.trim()).toBe('Required in Français, the fallback language.');
    expect(await axeViolations(element)).toEqual([]);

    form.controls.name.controls.fr.setValue('Support');
    form.controls.name.controls.en.setErrors({ api: 'errors.field.taken' });
    fixture.detectChanges();
    await fixture.whenStable();
    expect(Array.from(element.querySelectorAll('mat-error')).map((e) => e.textContent?.trim())).toEqual([
      'This value is already used.',
    ]);
  });
});
