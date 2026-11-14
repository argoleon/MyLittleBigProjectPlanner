import { Pipe, PipeTransform, inject } from '@angular/core';
import { AbstractControl, FormControl, FormGroup, ValidationErrors } from '@angular/forms';

import { DEFAULT_LANGUAGE, LANGUAGES, Language, LocaleService } from './locale.service';

/**
 * A translatable label as the API stores it: one text per language
 * (`{"fr": …, "en": …}`), any of them possibly missing.
 */
export type Translations = Readonly<Partial<Record<string, string>>>;

/** One field per language, as `ui-translatable-input` edits them. */
export type TranslationsForm = FormGroup<Record<Language, FormControl<string>>>;

/** Longest label, in characters, unless told otherwise. */
export const LABEL_MAX_LENGTH = 100;

/**
 * The label of `translations` in `language`, else in `fallback`, else in
 * any language; empty when there is none. Blank texts count as missing.
 */
export function resolveLabel(
  translations: Translations | null | undefined,
  language: Language,
  fallback: Language = DEFAULT_LANGUAGE,
): string {
  if (!translations) return '';
  const text = (lang: string) => translations[lang]?.trim() ?? '';
  return (
    text(language) ||
    text(fallback) ||
    Object.keys(translations)
      .map(text)
      .find((label) => label !== '') ||
    ''
  );
}

/** `required`, but blank text counts as missing too. */
export function notBlank(control: AbstractControl<string | null>): ValidationErrors | null {
  return control.value?.trim() ? null : { required: true };
}

/**
 * The fields of a translatable label: `fallback` is required (unless
 * `required` is false, for an optional label), every language at most
 * `maxLength` characters once trimmed.
 */
export function translationsForm(
  value: Translations = {},
  options: { fallback?: Language; maxLength?: number; required?: boolean } = {},
): TranslationsForm {
  const fallback = options.required === false ? null : (options.fallback ?? DEFAULT_LANGUAGE);
  const maxLength = options.maxLength ?? LABEL_MAX_LENGTH;
  const tooLong = (control: AbstractControl<string | null>): ValidationErrors | null =>
    (control.value?.trim().length ?? 0) > maxLength ? { too_long: true } : null;
  const controls = Object.fromEntries(
    LANGUAGES.map((language) => [
      language,
      new FormControl(value[language] ?? '', {
        nonNullable: true,
        validators: language === fallback ? [notBlank, tooLong] : [tooLong],
      }),
    ]),
  ) as Record<Language, FormControl<string>>;
  return new FormGroup(controls);
}

/** What the API expects: texts trimmed, blank ones left out. */
export function translationsValue(form: TranslationsForm): Record<string, string> {
  const value: Record<string, string> = {};
  for (const language of LANGUAGES) {
    const text = form.controls[language].value.trim();
    if (text) value[language] = text;
  }
  return value;
}

/** Whether `field` (e.g. `nameTranslations.fr`) names a language of a translatable label. */
export function translationLanguage(field: string, prefix: string): Language | null {
  const language = field.startsWith(`${prefix}.`) ? field.slice(prefix.length + 1) : null;
  return LANGUAGES.find((candidate) => candidate === language) ?? null;
}

/**
 * `{{ type.nameTranslations | translated }}`: a translatable label in the
 * user's language, else in the fallback language, else in any. Not pure:
 * it follows language changes.
 */
@Pipe({ name: 'translated', pure: false })
export class TranslatedPipe implements PipeTransform {
  private readonly locale = inject(LocaleService);

  transform(translations: Translations | null | undefined, fallback: Language = DEFAULT_LANGUAGE): string {
    return resolveLabel(translations, this.locale.language(), fallback);
  }
}
