import { TestBed } from '@angular/core/testing';
import { MatDialog } from '@angular/material/dialog';

import { provideMaterialDefaults } from '../theming/provide-material';
import { axeViolations } from '../testing/axe';
import { provideI18nTesting } from '../testing/i18n-testing';
import { ConfirmDialog, ConfirmDialogData } from './confirm-dialog';

describe('ConfirmDialog', () => {
  async function open(): Promise<{ closed: Promise<boolean | undefined>; container: () => HTMLElement }> {
    TestBed.configureTestingModule({ providers: [provideI18nTesting(), provideMaterialDefaults()] });
    const data: ConfirmDialogData = {
      titleKey: 'admin.members.removeTitle',
      messageKey: 'admin.members.removeMessage',
      confirmKey: 'admin.members.removeConfirm',
      params: { name: 'Lucas Bernard' },
    };
    const ref = TestBed.inject(MatDialog).open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, { data });
    const closed = new Promise<boolean | undefined>((resolve) => ref.afterClosed().subscribe(resolve));
    await new Promise((resolve) => setTimeout(resolve));
    TestBed.tick();
    return { closed, container: () => document.querySelector('mat-dialog-container') as HTMLElement };
  }

  it('asks, then closes with true when confirmed', async () => {
    const { closed, container } = await open();
    expect(container().querySelector('h2')?.textContent).toBe('Remove Lucas Bernard?');
    const buttons = Array.from(container().querySelectorAll('button'));
    expect(buttons.map((b) => b.textContent?.trim())).toEqual(['Cancel', 'Remove']);
    expect(await axeViolations(container())).toEqual([]);
    buttons[1].click();
    expect(await closed).toBe(true);
  });

  it('closes with false when cancelled', async () => {
    const { closed, container } = await open();
    (container().querySelector('button') as HTMLButtonElement).click();
    expect(await closed).toBe(false);
  });
});
