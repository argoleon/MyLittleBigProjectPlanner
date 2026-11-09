import { HttpClient } from '@angular/common/http';
import {
  EnvironmentProviders,
  Injectable,
  inject,
  isDevMode,
  makeEnvironmentProviders,
  provideAppInitializer,
} from '@angular/core';
import { TitleStrategy } from '@angular/router';
import { Translation, TranslocoLoader, provideTransloco } from '@jsverse/transloco';
import { Observable } from 'rxjs';

import { DEFAULT_LANGUAGE, LANGUAGES, LocaleService } from './locale.service';
import { APP_NAME_KEY, TranslatedTitleStrategy } from './translated-title.strategy';

/**
 * Loads `/i18n/<language>.json` at runtime. The files come from
 * `projects/ui/i18n/`, copied into each application by `angular.json`.
 */
@Injectable({ providedIn: 'root' })
export class I18nHttpLoader implements TranslocoLoader {
  private readonly http = inject(HttpClient);

  getTranslation(language: string): Observable<Translation> {
    return this.http.get<Translation>(`i18n/${language}.json`);
  }
}

/** Options of {@link provideI18n}. */
export interface I18nOptions {
  /** Translation key of the application name, used in page titles. */
  appNameKey?: string;
}

/**
 * Sets up translations (Transloco, French and English loaded at runtime),
 * the user's language and translated page titles (route `title` holds a
 * translation key). Needs `HttpClient`, which `provideApi()` provides.
 */
export function provideI18n(options: I18nOptions = {}): EnvironmentProviders {
  return makeEnvironmentProviders([
    { provide: APP_NAME_KEY, useValue: options.appNameKey ?? 'app.name' },
    { provide: TitleStrategy, useExisting: TranslatedTitleStrategy },
    provideTransloco({
      config: {
        availableLangs: [...LANGUAGES],
        defaultLang: DEFAULT_LANGUAGE,
        fallbackLang: DEFAULT_LANGUAGE,
        missingHandler: { useFallbackTranslation: true, logMissingKey: isDevMode() },
        reRenderOnLangChange: true,
        prodMode: !isDevMode(),
      },
      loader: I18nHttpLoader,
    }),
    // Picks the user's language before the first render.
    provideAppInitializer(() => {
      inject(LocaleService);
    }),
  ]);
}
