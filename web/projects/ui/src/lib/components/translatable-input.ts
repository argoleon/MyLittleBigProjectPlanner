import { Component, input } from '@angular/core';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { ReactiveFormsModule } from '@angular/forms';
import { TranslocoPipe } from '@jsverse/transloco';

import { DEFAULT_LANGUAGE, LANGUAGES, LANGUAGE_NAMES, Language } from '../i18n/locale.service';
import { LABEL_MAX_LENGTH, TranslationsForm } from '../i18n/translatable';

/**
 * A translatable label edited side by side, one outlined field per
 * language (mockup 12): the fallback language is required, the others
 * optional. It edits a group made by `translationsForm()`, so that it sits
 * in a reactive form like any field:
 *
 * ```ts
 * readonly form = new FormGroup({ name: translationsForm(config.nameTranslations) });
 * ```
 * ```html
 * <ui-translatable-input [group]="form.controls.name" [label]="'config.name' | transloco" />
 * …
 * body = { nameTranslations: translationsValue(form.controls.name) };
 * ```
 *
 * An error the API returns for a language (`nameTranslations.fr`) goes on
 * its control as `{ api: translationKey }`.
 */
@Component({
  selector: 'ui-translatable-input',
  imports: [MatFormFieldModule, MatInputModule, ReactiveFormsModule, TranslocoPipe],
  template: `
    @for (language of languages; track language) {
      <mat-form-field appearance="outline" class="language">
        <mat-label>
          {{ 'translatable.field' | transloco: { label: label(), language: names[language] } }}
        </mat-label>
        <input
          matInput
          [formControl]="group().controls[language]"
          [attr.lang]="language"
          [required]="language === fallback()"
          [attr.maxlength]="maxLength()"
        />
        @if (language === fallback()) {
          <mat-hint>{{ 'translatable.fallback' | transloco }}</mat-hint>
        } @else {
          <mat-hint>{{ 'translatable.optional' | transloco }}</mat-hint>
        }
        @if (group().controls[language].getError('api'); as key) {
          <mat-error>{{ key | transloco }}</mat-error>
        } @else if (group().controls[language].hasError('required')) {
          <mat-error>{{ 'translatable.required' | transloco: { language: names[language] } }}</mat-error>
        } @else if (group().controls[language].hasError('too_long')) {
          <mat-error>{{ 'errors.field.too_long' | transloco }}</mat-error>
        }
      </mat-form-field>
    }
  `,
  styleUrl: './translatable-input.scss',
})
export class TranslatableInput {
  /** The fields, from `translationsForm()`. */
  readonly group = input.required<TranslationsForm>();
  /** Translated name of what is labelled, e.g. "Name". */
  readonly label = input.required<string>();
  /** The language that must be filled in, whatever the user's; null for an optional label. */
  readonly fallback = input<Language | null>(DEFAULT_LANGUAGE);
  readonly maxLength = input(LABEL_MAX_LENGTH);

  protected readonly languages = LANGUAGES;
  protected readonly names = LANGUAGE_NAMES;
}
