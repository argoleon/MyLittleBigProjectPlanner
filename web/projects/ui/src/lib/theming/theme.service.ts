import { DOCUMENT, Injectable, inject, signal } from '@angular/core';

/** `system` follows the operating system; the others force a mode. */
export type ThemeMode = 'system' | 'light' | 'dark';

export const THEME_MODES: readonly ThemeMode[] = ['system', 'light', 'dark'];

const STORAGE_KEY = 'mlbpp.theme';

function isThemeMode(value: unknown): value is ThemeMode {
  return typeof value === 'string' && (THEME_MODES as readonly string[]).includes(value);
}

/**
 * Light or dark mode. The theme (`ui/styles/theme`) follows the system with
 * `color-scheme: light dark`; forcing a mode sets `<html data-theme>`, so
 * every `--mat-sys-*` and `--app-*` token switches without other code.
 */
@Injectable({ providedIn: 'root' })
export class ThemeService {
  private readonly document = inject(DOCUMENT);
  private readonly modeSignal = signal<ThemeMode>(this.storedMode());

  readonly mode = this.modeSignal.asReadonly();

  constructor() {
    this.apply(this.modeSignal());
  }

  /** Switches the mode and remembers the choice. */
  use(mode: ThemeMode): void {
    this.modeSignal.set(mode);
    this.apply(mode);
    try {
      this.document.defaultView?.localStorage.setItem(STORAGE_KEY, mode);
    } catch {
      // Storage disabled: the choice lasts until the page is reloaded.
    }
  }

  private apply(mode: ThemeMode): void {
    const root = this.document.documentElement;
    if (mode === 'system') {
      delete root.dataset['theme'];
    } else {
      root.dataset['theme'] = mode;
    }
  }

  private storedMode(): ThemeMode {
    try {
      const stored = this.document.defaultView?.localStorage.getItem(STORAGE_KEY);
      return isThemeMode(stored) ? stored : 'system';
    } catch {
      return 'system';
    }
  }
}
