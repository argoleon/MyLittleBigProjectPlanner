import { EnvironmentProviders, makeEnvironmentProviders } from '@angular/core';
import { MAT_FORM_FIELD_DEFAULT_OPTIONS } from '@angular/material/form-field';
import { MAT_ICON_DEFAULT_OPTIONS } from '@angular/material/icon';

/**
 * Angular Material defaults shared by every application: Material Symbols
 * Outlined icons (self-hosted, `material-symbols/outlined.css` in the
 * application styles) and outlined form fields, as in the mockups.
 */
export function provideMaterialDefaults(): EnvironmentProviders {
  return makeEnvironmentProviders([
    { provide: MAT_ICON_DEFAULT_OPTIONS, useValue: { fontSet: 'material-symbols-outlined' } },
    { provide: MAT_FORM_FIELD_DEFAULT_OPTIONS, useValue: { appearance: 'outline' } },
  ]);
}
