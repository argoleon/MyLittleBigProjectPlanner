import { DOCUMENT, Injectable, inject, signal } from '@angular/core';
import { TranslocoService } from '@jsverse/transloco';

/** Languages of the interface. French is the original language. */
export const LANGUAGES = ['fr', 'en'] as const;

export type Language = (typeof LANGUAGES)[number];

export const DEFAULT_LANGUAGE: Language = 'fr';

/** Name of each language in that language, for the language selector. */
export const LANGUAGE_NAMES: Readonly<Record<Language, string>> = { fr: 'Français', en: 'English' };

const STORAGE_KEY = 'mlbpp.language';

export function isLanguage(value: unknown): value is Language {
  return typeof value === 'string' && (LANGUAGES as readonly string[]).includes(value);
}

/**
 * Language of a browser's preferences: the first supported primary subtag of
 * `preferred` (e.g. `en-GB` → `en`), else the default language.
 */
export function languageFrom(preferred: readonly string[]): Language {
  for (const tag of preferred) {
    const primary = tag.toLowerCase().split('-')[0];
    if (isLanguage(primary)) {
      return primary;
    }
  }
  return DEFAULT_LANGUAGE;
}

/**
 * Locale used to format dates and numbers: the user's language, with the
 * region of their browser when it matches (`en` + `en-GB` → `en-GB`).
 */
export function formattingLocale(language: Language, preferred: readonly string[]): string {
  return preferred.find((tag) => tag.toLowerCase().split('-')[0] === language) ?? language;
}

/**
 * Language and time zone of the user.
 *
 * The language comes from the user's choice (kept in the browser), else from
 * the browser's preferences; it drives Transloco and `<html lang>`. The time
 * zone is the browser's until user profiles exist (L02).
 */
@Injectable({ providedIn: 'root' })
export class LocaleService {
  private readonly document = inject(DOCUMENT);
  private readonly transloco = inject(TranslocoService);
  private readonly preferred: readonly string[] = this.document.defaultView?.navigator.languages ?? [];

  private readonly languageSignal = signal<Language>(this.initialLanguage());

  /** Current language of the interface. */
  readonly language = this.languageSignal.asReadonly();

  /** IANA time zone in which instants are shown. */
  readonly timeZone = signal(Intl.DateTimeFormat().resolvedOptions().timeZone).asReadonly();

  constructor() {
    this.apply(this.languageSignal());
  }

  /** Locale for `Intl` formatting, e.g. `fr` or `en-GB`. */
  formattingLocale(): string {
    return formattingLocale(this.languageSignal(), this.preferred);
  }

  /** Switches the interface language and remembers the choice. */
  use(language: Language): void {
    this.languageSignal.set(language);
    this.apply(language);
    try {
      this.document.defaultView?.localStorage.setItem(STORAGE_KEY, language);
    } catch {
      // Storage disabled: the choice lasts until the page is reloaded.
    }
  }

  private apply(language: Language): void {
    this.transloco.setActiveLang(language);
    this.document.documentElement.lang = language;
  }

  private initialLanguage(): Language {
    try {
      const stored = this.document.defaultView?.localStorage.getItem(STORAGE_KEY);
      if (isLanguage(stored)) {
        return stored;
      }
    } catch {
      // Storage disabled: fall back on the browser's preferences.
    }
    return languageFrom(this.preferred);
  }
}
