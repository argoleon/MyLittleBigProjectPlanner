import { TestBed } from '@angular/core/testing';

import { ThemeService } from './theme.service';

describe('ThemeService', () => {
  beforeEach(() => {
    localStorage.clear();
    delete document.documentElement.dataset['theme'];
  });

  it('follows the system by default', () => {
    const theme = TestBed.inject(ThemeService);
    expect(theme.mode()).toBe('system');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('forces light or dark with <html data-theme>, then back to the system', () => {
    const theme = TestBed.inject(ThemeService);
    theme.use('dark');
    expect(document.documentElement.dataset['theme']).toBe('dark');
    theme.use('light');
    expect(document.documentElement.dataset['theme']).toBe('light');
    theme.use('system');
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
  });

  it('remembers the choice', () => {
    TestBed.inject(ThemeService).use('dark');
    expect(localStorage.getItem('mlbpp.theme')).toBe('dark');
    TestBed.resetTestingModule();
    delete document.documentElement.dataset['theme'];

    const restored = TestBed.inject(ThemeService);
    expect(restored.mode()).toBe('dark');
    expect(document.documentElement.dataset['theme']).toBe('dark');
  });

  it('ignores an invalid stored mode', () => {
    localStorage.setItem('mlbpp.theme', 'sepia');
    expect(TestBed.inject(ThemeService).mode()).toBe('system');
  });
});
