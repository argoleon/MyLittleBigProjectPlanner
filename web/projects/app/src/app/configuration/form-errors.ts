import { AbstractControl, FormGroup } from '@angular/forms';
import { errorTranslationKey, fieldErrorTranslationKey, isApiProblem, translationLanguage } from 'ui';

/**
 * Puts the field errors of an API problem on the controls they name
 * (`{ api: translationKey }`): a plain field (`key`), or a translatable
 * label as a whole (`labelTranslations`) or in one language
 * (`labelTranslations.fr`). Returns the translation key to show above the
 * form when something is left (a problem without field errors, or a field
 * no control holds); null when every error found its place.
 */
export function placeApiErrors(error: unknown, controls: Readonly<Record<string, AbstractControl>>): string | null {
  if (!isApiProblem(error)) return errorTranslationKey('unknown');
  let left = error.fieldErrors.length === 0;
  for (const fieldError of error.fieldErrors) {
    const key = fieldErrorTranslationKey(fieldError.code);
    const targets = targetsOf(fieldError.field, controls);
    for (const target of targets) {
      target.setErrors({ api: key });
      target.markAsTouched();
    }
    left ||= targets.length === 0;
  }
  return left ? error.translationKey : null;
}

function targetsOf(field: string, controls: Readonly<Record<string, AbstractControl>>): AbstractControl[] {
  const direct = controls[field];
  if (direct instanceof FormGroup) return Object.values(direct.controls);
  if (direct) return [direct];
  const [prefix] = field.split('.');
  const group = controls[prefix];
  const language = translationLanguage(field, prefix);
  if (group instanceof FormGroup && language) {
    const control = group.controls[language] as AbstractControl | undefined;
    return control ? [control] : [];
  }
  return [];
}
