import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';
import { provideMaterialDefaults } from 'ui';
import { provideI18nTesting } from 'ui/testing';

import { NODE_ICONS, parentNames, parseWeight } from './node-links';
import { WeightField } from './weight-field';

describe('parseWeight', () => {
  it('reads whole percentages from 0 to 100', () => {
    expect(parseWeight('40')).toBe(40);
    expect(parseWeight(' 0 ')).toBe(0);
    expect(parseWeight('100')).toBe(100);
    expect(parseWeight('40 %')).toBe(40);
    expect(parseWeight('40%')).toBe(40);
  });

  it('refuses anything else', () => {
    for (const text of ['', '101', '-1', '4.5', '1e2', 'forty', '%', '0040']) {
      expect(parseWeight(text), text).toBeNull();
    }
  });
});

describe('parentNames and NODE_ICONS', () => {
  it('names the parents for context, and gives each kind an icon', () => {
    expect(parentNames({ parents: [{ name: 'Digital' }, { name: 'Compliance' }] })).toBe('Digital, Compliance');
    expect(parentNames({ parents: [] })).toBe('');
    expect(new Set(Object.values(NODE_ICONS)).size).toBe(3);
  });
});

@Component({
  imports: [WeightField],
  template: `<app-weight-field [weight]="weight()" parentName="Compliance" (weightChange)="changes.push($event)" />`,
})
class Host {
  readonly weight = signal(100);
  readonly changes: number[] = [];
}

describe('WeightField', () => {
  async function setUp() {
    TestBed.configureTestingModule({ providers: [provideI18nTesting(), provideMaterialDefaults()] });
    const fixture = TestBed.createComponent(Host);
    await fixture.whenStable();
    const input = fixture.nativeElement.querySelector('input') as HTMLInputElement;
    const enter = async (value: string) => {
      input.value = value;
      input.dispatchEvent(new Event('input'));
      input.dispatchEvent(new Event('change'));
      input.dispatchEvent(new Event('blur'));
      await fixture.whenStable();
    };
    return { fixture, input, enter, host: fixture.componentInstance };
  }

  it('shows the weighting, named after the parent', async () => {
    const { input, fixture } = await setUp();
    expect(input.value).toBe('100');
    expect(input.getAttribute('aria-label')).toBe('Weighting under Compliance');
    expect(fixture.nativeElement.textContent).toContain('%');
  });

  it('emits a valid new weighting, and only that', async () => {
    const { enter, host, fixture } = await setUp();
    await enter('40');
    expect(host.changes).toEqual([40]);
    await enter('140');
    expect(host.changes).toEqual([40]);
    expect(fixture.nativeElement.querySelector('mat-error')?.textContent?.trim()).toBe(
      'The value is out of the allowed range.',
    );
    // The saved weighting comes back: the field follows it.
    host.weight.set(40);
    await fixture.whenStable();
    expect(fixture.nativeElement.querySelector('input').value).toBe('40');
    await enter('40');
    expect(host.changes).toEqual([40]);
  });
});
