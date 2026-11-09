import {
  EnvironmentProviders,
  importProvidersFrom,
  inject,
  makeEnvironmentProviders,
  provideAppInitializer,
} from '@angular/core';
import { TitleStrategy } from '@angular/router';
import { TranslocoTestingModule } from '@jsverse/transloco';

import en from '../../../i18n/en.json';
import fr from '../../../i18n/fr.json';
import { DEFAULT_LANGUAGE, LANGUAGES, LocaleService } from '../i18n/locale.service';
import { APP_NAME_KEY, TranslatedTitleStrategy } from '../i18n/translated-title.strategy';

/**
 * `provideI18n()` for tests: the real FR and EN translations, loaded
 * synchronously, the user's language (jsdom's browser language is English)
 * and translated page titles.
 */
export function provideI18nTesting(options: { appNameKey?: string } = {}): EnvironmentProviders {
  return makeEnvironmentProviders([
    { provide: APP_NAME_KEY, useValue: options.appNameKey ?? 'app.name' },
    { provide: TitleStrategy, useExisting: TranslatedTitleStrategy },
    importProvidersFrom(
    TranslocoTestingModule.forRoot({
      langs: { fr, en },
      translocoConfig: {
        availableLangs: [...LANGUAGES],
        defaultLang: DEFAULT_LANGUAGE,
        fallbackLang: DEFAULT_LANGUAGE,
        reRenderOnLangChange: true,
      },
      preloadLangs: true,
    }),
  ),
    provideAppInitializer(() => {
      inject(LocaleService);
    }),
  ]);
}
