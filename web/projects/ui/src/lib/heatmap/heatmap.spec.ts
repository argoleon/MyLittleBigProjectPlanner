/**
 * Tests of the canvas heatmap (L08-US06): which rows and columns are in
 * view, the keyboard moves of the focused cell and the cell under a point
 * (unit); and the component (integration): a focused cell said by the
 * live region as the arrows move it, Enter and a click opening a cell,
 * the grid scrolled to the focused cell, accessibility.
 */
import { Component, signal } from '@angular/core';
import { TestBed } from '@angular/core/testing';

import { axeViolations } from '../testing/axe';
import { HEATMAP_SIZES, HeatmapCellData, HeatmapGrid, HeatmapPosition, cellAt, moveFocus, revealScroll, visibleRange } from './heatmap';

describe('heatmap geometry', () => {
  it('draws only the rows or columns in view', () => {
    expect(visibleRange(0, 560, 56, 200)).toEqual({ first: 0, last: 9 });
    expect(visibleRange(57, 560, 56, 200)).toEqual({ first: 1, last: 11 });
    expect(visibleRange(11000, 560, 56, 200)).toEqual({ first: 196, last: 199 });
    expect(visibleRange(0, 560, 56, 0)).toEqual({ first: 0, last: -1 });
  });

  it('moves the focus with the arrows, Home, End and the page keys, within the grid', () => {
    const at = { row: 3, column: 2 };
    expect(moveFocus('ArrowRight', false, at, 10, 4, 5)).toEqual({ row: 3, column: 3 });
    expect(moveFocus('ArrowLeft', false, { row: 3, column: 0 }, 10, 4, 5)).toEqual({ row: 3, column: 0 });
    expect(moveFocus('ArrowDown', false, { row: 9, column: 2 }, 10, 4, 5)).toEqual({ row: 9, column: 2 });
    expect(moveFocus('ArrowUp', false, at, 10, 4, 5)).toEqual({ row: 2, column: 2 });
    expect(moveFocus('Home', false, at, 10, 4, 5)).toEqual({ row: 3, column: 0 });
    expect(moveFocus('End', false, at, 10, 4, 5)).toEqual({ row: 3, column: 3 });
    expect(moveFocus('Home', true, at, 10, 4, 5)).toEqual({ row: 0, column: 0 });
    expect(moveFocus('End', true, at, 10, 4, 5)).toEqual({ row: 9, column: 3 });
    expect(moveFocus('PageDown', false, at, 10, 4, 5)).toEqual({ row: 8, column: 2 });
    expect(moveFocus('PageUp', false, at, 10, 4, 5)).toEqual({ row: 0, column: 2 });
    expect(moveFocus('a', false, at, 10, 4, 5)).toBeNull();
  });

  it('scrolls just enough to keep the focused cell in view', () => {
    const viewport = { width: 360, height: 280 };
    expect(revealScroll({ row: 2, column: 1 }, { top: 0, left: 0 }, viewport)).toEqual({ top: 0, left: 0 });
    // Below and right of the view: scrolled until its far edges show.
    expect(revealScroll({ row: 10, column: 8 }, { top: 0, left: 0 }, viewport)).toEqual({ top: 11 * 56 - 280, left: 9 * 72 - 360 });
    // Above and left: scrolled to its near edges.
    expect(revealScroll({ row: 1, column: 0 }, { top: 300, left: 200 }, viewport)).toEqual({ top: 56, left: 0 });
  });

  it('finds the cell under a point, none over the headers', () => {
    const { rowHeaderWidth, headerHeight, columnWidth, rowHeight } = HEATMAP_SIZES;
    expect(cellAt(rowHeaderWidth + 1, headerHeight + 1, 3, 3)).toEqual({ row: 0, column: 0 });
    expect(cellAt(rowHeaderWidth + columnWidth * 2 + 5, headerHeight + rowHeight + 5, 3, 3)).toEqual({ row: 1, column: 2 });
    expect(cellAt(10, headerHeight + 10, 3, 3)).toBeNull();
    expect(cellAt(rowHeaderWidth + 10, 10, 3, 3)).toBeNull();
    expect(cellAt(rowHeaderWidth + columnWidth * 5, headerHeight + 1, 3, 3)).toBeNull();
  });
});

const cell = (text: string, level: HeatmapCellData['level'] = 'medium'): HeatmapCellData => ({ level, text, description: `${text} of the capacity` });

@Component({
  imports: [HeatmapGrid],
  template: `
    <ui-heatmap
      style="width: 600px"
      [rows]="rows"
      [columns]="columns"
      [cells]="cells"
      label="Workload of the team"
      instructions="Arrows move between cells, Enter opens one."
      corner="Resource"
      (activate)="opened.set($event)"
    />
  `,
})
class Host {
  readonly rows = Array.from({ length: 30 }, (_, i) => ({ label: `Resource ${i}`, sublabel: 'Dev back' }));
  readonly columns = Array.from({ length: 12 }, (_, i) => ({ label: `W${45 + i}`, sublabel: 'Nov 2–8' }));
  readonly cells = this.rows.map((_, r) => this.columns.map((_, c) => cell(`${r * 10 + c} %`, r === 0 && c === 2 ? 'over' : 'medium')));
  readonly opened = signal<HeatmapPosition | null>(null);
}

describe('Heatmap', () => {
  async function open() {
    const fixture = TestBed.createComponent(Host);
    document.body.appendChild(fixture.nativeElement);
    fixture.detectChanges();
    await new Promise((resolve) => setTimeout(resolve, 50));
    const host = fixture.nativeElement as HTMLElement;
    const grid = host.querySelector<HTMLElement>('.scroller')!;
    const live = () => host.querySelector('[aria-live]')?.textContent?.trim();
    const key = async (name: string, ctrl = false) => {
      grid.dispatchEvent(new KeyboardEvent('keydown', { key: name, ctrlKey: ctrl, bubbles: true }));
      fixture.detectChanges();
      await new Promise((resolve) => setTimeout(resolve, 20));
    };
    return { fixture, host, grid, live, key };
  }

  it('is one focusable control, named and described, its canvas hidden from screen readers', async () => {
    const { host, grid, fixture } = await open();
    expect(grid.getAttribute('tabindex')).toBe('0');
    expect(grid.getAttribute('aria-label')).toBe('Workload of the team');
    const described = host.querySelector(`#${grid.getAttribute('aria-describedby')}`);
    expect(described?.textContent).toContain('Arrows move between cells');
    expect(host.querySelector('canvas')?.getAttribute('aria-hidden')).toBe('true');
    // Only what is in view is drawn: the spacer gives the grid its size.
    const spacer = host.querySelector<HTMLElement>('.spacer')!;
    expect(spacer.style.height).toBe(`${HEATMAP_SIZES.headerHeight + 30 * HEATMAP_SIZES.rowHeight}px`);
    expect(await axeViolations(fixture.nativeElement)).toEqual([]);
    fixture.nativeElement.remove();
  });

  it('says the focused cell as the arrows move it, and opens it with Enter', async () => {
    const { grid, live, key, fixture } = await open();
    grid.focus();
    fixture.detectChanges();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(live()).toBe('Resource 0, W45 Nov 2–8: 0 % of the capacity');
    await key('ArrowRight');
    await key('ArrowRight');
    expect(live()).toBe('Resource 0, W47 Nov 2–8: 2 % of the capacity');
    await key('Enter');
    expect(fixture.componentInstance.opened()).toEqual({ row: 0, column: 2 });
    // Far down: the grid scrolls to keep the focused cell in view.
    await key('End', true);
    expect(live()).toBe('Resource 29, W56 Nov 2–8: 301 % of the capacity');
    await key('PageUp');
    expect(fixture.componentInstance.opened()).toEqual({ row: 0, column: 2 });
    fixture.nativeElement.remove();
  });

  it('opens the cell clicked, not a header', async () => {
    const { grid, fixture } = await open();
    const box = grid.getBoundingClientRect();
    const { rowHeaderWidth, headerHeight, columnWidth, rowHeight } = HEATMAP_SIZES;
    grid.dispatchEvent(new MouseEvent('click', { clientX: box.left + 20, clientY: box.top + headerHeight + 20, bubbles: true }));
    expect(fixture.componentInstance.opened()).toBeNull();
    grid.dispatchEvent(
      new MouseEvent('click', {
        clientX: box.left + rowHeaderWidth + columnWidth + 10,
        clientY: box.top + headerHeight + rowHeight * 2 + 10,
        bubbles: true,
      }),
    );
    expect(fixture.componentInstance.opened()).toEqual({ row: 2, column: 1 });
    fixture.nativeElement.remove();
  });
});
