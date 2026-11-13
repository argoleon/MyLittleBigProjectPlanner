import { TestBed } from '@angular/core/testing';
import { axeViolations, provideI18nTesting } from 'ui/testing';

import { VeiledName, isVeiled } from './veiled';

describe('isVeiled', () => {
  it('tells a veiled node from a node and from a link', () => {
    expect(isVeiled({ veiled: true, id: 'x', type: 'project', parents: [] })).toBe(true);
    expect(isVeiled({ id: 'n', type: 'project', name: 'Identity platform', confidential: true })).toBe(false);
    expect(isVeiled({ id: 'n', type: 'project', name: 'Identity platform', weightPercent: 100 })).toBe(false);
  });
});

describe('VeiledName', () => {
  it('shows a labelled lock and "Confidential project", never a name', async () => {
    TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
    const fixture = TestBed.createComponent(VeiledName);
    await fixture.whenStable();
    const element = fixture.nativeElement as HTMLElement;
    const lock = element.querySelector('mat-icon')!;
    expect(lock.getAttribute('role')).toBe('img');
    expect(lock.getAttribute('aria-label')).toBe('Locked');
    expect(lock.getAttribute('aria-hidden')).toBe('false');
    expect(element.querySelector('.name')?.textContent).toBe('Confidential project');
    expect(await axeViolations(element)).toEqual([]);
  });
});
