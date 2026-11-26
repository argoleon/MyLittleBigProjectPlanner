/**
 * Unit tests of the heatmap model (L08-US06): the range of each scale and
 * how it moves, the cell levels with the tolerance, the column headers,
 * and the cells built from the API's rows: text, hatching, description
 * for screen readers, the averages and the overload count; in English and
 * French.
 */
import { TestBed } from '@angular/core/testing';
import { TranslocoService } from '@jsverse/transloco';
import { Heatmap as HeatmapData, LocaleService } from 'ui';
import { provideI18nTesting } from 'ui/testing';

import { anchorOf, buildModel, columnOf, levelOf, percentOf, rangeOf, shift } from './heatmap-model';

const DATA: HeatmapData = {
  scale: 'week',
  periods: [
    { start: '2026-11-16', end: '2026-11-22' },
    { start: '2026-11-23', end: '2026-11-29' },
  ],
  rows: [
    {
      resource: { id: 'r1', kind: 'person', displayName: 'Camille Martin', jobTitle: 'Dev back', linked: true },
      load: [2352, 1050],
      provisional: [0, 0],
      hidden: [0, 600],
      capacity: [2100, 2100],
      scheduled: [2100, 2100],
    },
    {
      resource: { id: 'r2', kind: 'generic', displayName: 'Rust developer', linked: false },
      load: [840, 0],
      provisional: [840, 0],
      hidden: [0, 0],
      capacity: [2100, 0],
      scheduled: [2100, 2100],
    },
  ],
  tolerancePercent: 0,
  includeProvisional: true,
};

function translator(lang: 'en' | 'fr') {
  localStorage.clear();
  TestBed.configureTestingModule({ providers: [provideI18nTesting()] });
  TestBed.inject(LocaleService).use(lang);
  const transloco = TestBed.inject(TranslocoService);
  return (key: string, params?: Record<string, string>) => transloco.translate(key, params);
}

describe('the heatmap model', () => {
  afterEach(() => {
    localStorage.clear();
    TestBed.resetTestingModule();
  });

  it('reads eight weeks from a Monday, four weeks of days, six months, and moves by a screen', () => {
    expect(anchorOf('week', '2026-11-26')).toBe('2026-11-23');
    expect(anchorOf('month', '2026-11-26')).toBe('2026-11-01');
    expect(rangeOf('week', '2026-11-23')).toEqual({ from: '2026-11-23', to: '2027-01-17' });
    expect(rangeOf('day', '2026-11-23')).toEqual({ from: '2026-11-23', to: '2026-12-20' });
    expect(rangeOf('month', '2026-11-01')).toEqual({ from: '2026-11-01', to: '2027-04-30' });
    expect(shift('week', '2026-11-23', -1)).toBe('2026-09-28');
    expect(shift('day', '2026-11-23', 1)).toBe('2026-12-21');
    expect(shift('month', '2026-11-01', 1)).toBe('2027-05-01');
  });

  it('gives each cell its level, the overload beyond the tolerance', () => {
    expect(levelOf(0, 2100, 2100, 0)).toBe('free');
    expect(levelOf(840, 2100, 2100, 0)).toBe('low');
    expect(levelOf(1470, 2100, 2100, 0)).toBe('medium');
    expect(levelOf(1995, 2100, 2100, 0)).toBe('high');
    expect(levelOf(2100, 2100, 2100, 0)).toBe('full');
    expect(levelOf(2352, 2100, 2100, 0)).toBe('over');
    expect(levelOf(2352, 2100, 2100, 15)).toBe('full');
    expect(levelOf(0, 0, 2100, 0)).toBe('absent');
    expect(levelOf(0, 0, 0, 0)).toBe('off');
    expect(levelOf(60, 0, 0, 0)).toBe('over');
    expect([percentOf(2352, 2100), percentOf(60, 0)]).toEqual([112, null]);
  });

  it('builds the cells, their descriptions and the averages, in English', () => {
    const t = translator('en');
    expect(columnOf('week', DATA.periods[0], 'en', t)).toEqual({ label: 'W47', sublabel: '16–Nov 22' });
    expect(columnOf('day', DATA.periods[0], 'en', t)).toEqual({ label: 'Mon', sublabel: 'Nov 16' });
    expect(columnOf('month', DATA.periods[0], 'en', t)).toEqual({ label: 'Nov', sublabel: '2026' });
    const model = buildModel(DATA, 'en', t);
    expect(model.rows.map((r) => [r.label, r.sublabel])).toEqual([
      ['Camille Martin', 'Dev back'],
      ['Rust developer', 'Generic resource'],
      ['Average', undefined],
    ]);
    expect(model.overloads).toBe(1);
    const [camille, dev, average] = model.cells;
    // AC2: 112% overloads.
    expect(camille[0]).toEqual({
      level: 'over',
      text: '112%',
      subtext: undefined,
      provisional: false,
      hidden: false,
      description: '112%, 39 h 12 min planned for 35 h available, overloaded',
    });
    expect([camille[1].text, camille[1].subtext, camille[1].hidden]).toEqual(['50%', 'confidential', true]);
    expect(camille[1].description).toBe('50%, 17 h 30 min planned for 35 h available, including 10 h on tickets you do not see');
    expect([dev[0].level, dev[0].subtext, dev[0].provisional]).toEqual(['low', 'provisional', true]);
    expect([dev[1].level, dev[1].text, dev[1].description]).toEqual(['absent', 'Away', 'away the whole period']);
    // Averages over the cells with capacity.
    expect(average.map((c) => [c.level, c.text])).toEqual([
      ['summary', '76%'],
      ['summary', '50%'],
    ]);
    expect(buildModel({ ...DATA, rows: [] }, 'en', t).rows).toEqual([]);
  });

  it('says them in French', () => {
    const t = translator('fr');
    const model = buildModel(DATA, 'fr', t);
    expect(model.columns[0].label).toBe('S47');
    // Intl puts a no-break space before % in French.
    expect(model.cells[0][0].text).toMatch(/^112\s%$/);
    expect(model.cells[0][0].description).toMatch(/^112\s%, 39 h 12 planifiées pour 35 h disponibles, en surcharge$/);
    expect(model.cells[1][1].text).toBe('Absence');
  });
});
