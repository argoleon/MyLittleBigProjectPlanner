import {
  AfterViewInit,
  Component,
  DestroyRef,
  ElementRef,
  computed,
  effect,
  inject,
  input,
  output,
  signal,
  viewChild,
} from '@angular/core';

/**
 * How busy a cell is: off (no working day), absent (working days all
 * taken by absences), free, the four occupation levels, overloaded; and
 * `summary` for a row of averages, drawn without a fill.
 */
export type HeatmapLevel = 'off' | 'absent' | 'free' | 'low' | 'medium' | 'high' | 'full' | 'over' | 'summary';

/** A row header: a resource, or the averages. */
export interface HeatmapRowHeader {
  readonly label: string;
  readonly sublabel?: string;
}

/** A column header: a period. */
export interface HeatmapColumnHeader {
  readonly label: string;
  readonly sublabel?: string;
  /** A short note under it, such as a public holiday. */
  readonly note?: string;
}

/** What a cell shows. */
export interface HeatmapCellData {
  readonly level: HeatmapLevel;
  /** Its text, such as `82 %` or `Leave`. */
  readonly text: string;
  /** A second line, such as `provisional`. */
  readonly subtext?: string;
  /** Hatched: provisional load in it. */
  readonly provisional?: boolean;
  /** Hatched grey: load on tickets the reader does not see. */
  readonly hidden?: boolean;
  /** What a screen reader says of it, after its row and column. */
  readonly description: string;
}

/** A cell, by its row and column. */
export interface HeatmapPosition {
  readonly row: number;
  readonly column: number;
}

/** Sizes of the grid, in CSS pixels. */
export const HEATMAP_SIZES = { rowHeight: 56, columnWidth: 72, headerHeight: 64, rowHeaderWidth: 240, gap: 4 } as const;

/** The rows or columns at least partly in view: first and last index, included. */
export function visibleRange(scroll: number, viewport: number, size: number, count: number): { first: number; last: number } {
  if (count === 0 || viewport <= 0) return { first: 0, last: -1 };
  const first = Math.max(0, Math.floor(scroll / size));
  const last = Math.min(count - 1, Math.floor((scroll + viewport - 1) / size));
  return { first, last };
}

/**
 * Where the focus goes from `at` on `key` in a grid of `rows` × `columns`:
 * arrows move by one, Home and End to the row's ends (with Ctrl, to the
 * grid's), Page Up and Down by `page` rows. `null` for other keys.
 */
export function moveFocus(
  key: string,
  ctrl: boolean,
  at: HeatmapPosition,
  rows: number,
  columns: number,
  page: number,
): HeatmapPosition | null {
  const clamp = (value: number, count: number) => Math.min(Math.max(value, 0), Math.max(count - 1, 0));
  switch (key) {
    case 'ArrowUp':
      return { row: clamp(at.row - 1, rows), column: at.column };
    case 'ArrowDown':
      return { row: clamp(at.row + 1, rows), column: at.column };
    case 'ArrowLeft':
      return { row: at.row, column: clamp(at.column - 1, columns) };
    case 'ArrowRight':
      return { row: at.row, column: clamp(at.column + 1, columns) };
    case 'Home':
      return ctrl ? { row: 0, column: 0 } : { row: at.row, column: 0 };
    case 'End':
      return ctrl ? { row: clamp(rows - 1, rows), column: clamp(columns - 1, columns) } : { row: at.row, column: clamp(columns - 1, columns) };
    case 'PageUp':
      return { row: clamp(at.row - page, rows), column: at.column };
    case 'PageDown':
      return { row: clamp(at.row + page, rows), column: at.column };
    default:
      return null;
  }
}

/**
 * The scroll position that keeps `at` in view beside the headers, from
 * `scroll` in a grid area of `viewport` (both without the headers).
 */
export function revealScroll(
  at: HeatmapPosition,
  scroll: { top: number; left: number },
  viewport: { width: number; height: number },
): { top: number; left: number } {
  const { rowHeight, columnWidth } = HEATMAP_SIZES;
  const top = at.row * rowHeight;
  const left = at.column * columnWidth;
  const keep = (start: number, size: number, current: number, room: number) =>
    start < current ? start : start + size > current + room ? start + size - room : current;
  return {
    top: keep(top, rowHeight, scroll.top, viewport.height),
    left: keep(left, columnWidth, scroll.left, viewport.width),
  };
}

/** The cell under a point of the grid's content (headers excluded); `null` outside the cells. */
export function cellAt(x: number, y: number, rows: number, columns: number): HeatmapPosition | null {
  const { rowHeight, columnWidth, headerHeight, rowHeaderWidth } = HEATMAP_SIZES;
  if (x < rowHeaderWidth || y < headerHeight) return null;
  const column = Math.floor((x - rowHeaderWidth) / columnWidth);
  const row = Math.floor((y - headerHeight) / rowHeight);
  return row < rows && column < columns ? { row, column } : null;
}

/** The colors of the grid, read from the theme's tokens. */
interface Palette {
  readonly surface: string;
  readonly onSurface: string;
  readonly onSurfaceVariant: string;
  readonly outline: string;
  readonly focus: string;
  readonly fills: Record<Exclude<HeatmapLevel, 'summary'>, { fill: string; text: string }>;
  readonly overOutline: string;
  readonly provisional: string;
  readonly hidden: string;
  readonly holiday: string;
  readonly label: string;
  readonly small: string;
  readonly strong: string;
}

/**
 * A heatmap of rows (resources) by columns (periods), drawn on a canvas
 * (specification §8, L08-US06): only the rows and columns in view are
 * drawn, the row headers and column headers stay in place while the grid
 * scrolls. Colors come from the `--app-load-*` tokens, read at runtime, so
 * light and dark modes follow the theme; provisional load and load on
 * tickets the reader does not see are hatched.
 *
 * The grid takes the focus as one control: the arrows move a focused cell
 * (Home/End, Ctrl+Home/End, Page Up/Down), Enter or Space opens it, and a
 * live region says the focused cell. A click opens the cell under it. An
 * equivalent table view is the screen's job.
 */
@Component({
  selector: 'ui-heatmap',
  template: `
    <div
      #scroller
      class="scroller"
      tabindex="0"
      role="application"
      [attr.aria-label]="label()"
      [attr.aria-describedby]="instructionsId"
      (scroll)="schedule()"
      (keydown)="onKey($event)"
      (click)="onClick($event)"
      (focus)="onFocus()"
    >
      <div class="spacer" [style.width.px]="contentWidth()" [style.height.px]="contentHeight()"></div>
    </div>
    <canvas #canvas class="canvas" aria-hidden="true"></canvas>
    <p class="cdk-visually-hidden" [id]="instructionsId">{{ instructions() }}</p>
    <p class="cdk-visually-hidden" aria-live="polite" aria-atomic="true">{{ announcement() }}</p>
    <span #fontLabel class="font-probe label" aria-hidden="true"></span>
    <span #fontSmall class="font-probe small" aria-hidden="true"></span>
    <span #fontStrong class="font-probe strong" aria-hidden="true"></span>
  `,
  styleUrl: './heatmap.scss',
})
export class HeatmapGrid implements AfterViewInit {
  /** The rows, top to bottom. */
  readonly rows = input.required<readonly HeatmapRowHeader[]>();
  /** The columns, left to right. */
  readonly columns = input.required<readonly HeatmapColumnHeader[]>();
  /** The cells, by row then column. */
  readonly cells = input.required<readonly (readonly HeatmapCellData[])[]>();
  /** The grid's accessible name. */
  readonly label = input.required<string>();
  /** How to use it with the keyboard, for screen readers. */
  readonly instructions = input('');
  /** Title of the row headers' column. */
  readonly corner = input('');
  /** A cell opened: clicked, or Enter on the focused one. */
  readonly activate = output<HeatmapPosition>();

  private readonly scroller = viewChild.required<ElementRef<HTMLDivElement>>('scroller');
  private readonly canvas = viewChild.required<ElementRef<HTMLCanvasElement>>('canvas');
  private readonly fontLabel = viewChild.required<ElementRef<HTMLElement>>('fontLabel');
  private readonly fontSmall = viewChild.required<ElementRef<HTMLElement>>('fontSmall');
  private readonly fontStrong = viewChild.required<ElementRef<HTMLElement>>('fontStrong');
  private readonly host = inject<ElementRef<HTMLElement>>(ElementRef);
  private readonly destroyRef = inject(DestroyRef);

  private static nextId = 0;
  protected readonly instructionsId = `ui-heatmap-instructions-${HeatmapGrid.nextId++}`;

  /** The focused cell. */
  readonly focused = signal<HeatmapPosition>({ row: 0, column: 0 });
  /** What the live region says. */
  protected readonly announcement = signal('');
  protected readonly contentWidth = computed(
    () => HEATMAP_SIZES.rowHeaderWidth + this.columns().length * HEATMAP_SIZES.columnWidth,
  );
  protected readonly contentHeight = computed(() => HEATMAP_SIZES.headerHeight + this.rows().length * HEATMAP_SIZES.rowHeight);
  private palette: Palette | null = null;
  private frame = 0;
  private focusedInside = false;

  constructor() {
    effect(() => {
      // Redraw when the data or the focus changes.
      this.rows();
      this.columns();
      this.cells();
      this.focused();
      this.schedule();
    });
  }

  ngAfterViewInit(): void {
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => this.schedule());
    observer?.observe(this.scroller().nativeElement);
    // The theme may switch between light and dark.
    const scheme = window.matchMedia?.('(prefers-color-scheme: dark)');
    const repaint = () => {
      this.palette = null;
      this.schedule();
    };
    scheme?.addEventListener?.('change', repaint);
    this.destroyRef.onDestroy(() => {
      observer?.disconnect();
      scheme?.removeEventListener?.('change', repaint);
      cancelAnimationFrame(this.frame);
    });
    this.schedule();
  }

  /** Draws on the next frame. */
  schedule(): void {
    cancelAnimationFrame(this.frame);
    this.frame = requestAnimationFrame(() => this.draw());
  }

  /** The text a screen reader says of `at`. */
  describe(at: HeatmapPosition): string {
    const row = this.rows()[at.row];
    const column = this.columns()[at.column];
    const cell = this.cells()[at.row]?.[at.column];
    if (!row || !column || !cell) return '';
    const period = [column.label, column.sublabel].filter(Boolean).join(' ');
    return `${row.label}, ${period}: ${cell.description}`;
  }

  protected onFocus(): void {
    this.focusedInside = true;
    this.announcement.set(this.describe(this.focused()));
    this.schedule();
  }

  protected onKey(event: KeyboardEvent): void {
    const rows = this.rows().length;
    const columns = this.columns().length;
    if (rows === 0 || columns === 0) return;
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      this.activate.emit(this.focused());
      return;
    }
    const viewport = this.scroller().nativeElement.clientHeight - HEATMAP_SIZES.headerHeight;
    const page = Math.max(1, Math.floor(viewport / HEATMAP_SIZES.rowHeight));
    const next = moveFocus(event.key, event.ctrlKey || event.metaKey, this.focused(), rows, columns, page);
    if (!next) return;
    event.preventDefault();
    this.focused.set(next);
    this.reveal(next);
    this.announcement.set(this.describe(next));
  }

  protected onClick(event: MouseEvent): void {
    const element = this.scroller().nativeElement;
    const box = element.getBoundingClientRect();
    const x = event.clientX - box.left;
    const y = event.clientY - box.top;
    // Headers stay in place: a point over them is no cell.
    if (x < HEATMAP_SIZES.rowHeaderWidth || y < HEATMAP_SIZES.headerHeight) return;
    const at = cellAt(x + element.scrollLeft, y + element.scrollTop, this.rows().length, this.columns().length);
    if (!at) return;
    this.focused.set(at);
    this.announcement.set(this.describe(at));
    this.activate.emit(at);
  }

  /** Scrolls so that `at` is in view, beside the headers. */
  private reveal(at: HeatmapPosition): void {
    const element = this.scroller().nativeElement;
    const next = revealScroll(
      at,
      { top: element.scrollTop, left: element.scrollLeft },
      {
        width: element.clientWidth - HEATMAP_SIZES.rowHeaderWidth,
        height: element.clientHeight - HEATMAP_SIZES.headerHeight,
      },
    );
    element.scrollTop = next.top;
    element.scrollLeft = next.left;
  }

  private readPalette(): Palette {
    const style = getComputedStyle(this.host.nativeElement);
    const token = (name: string) => style.getPropertyValue(name).trim();
    const pair = (fill: string, text: string) => ({ fill: token(fill), text: token(text) });
    const font = (probe: ElementRef<HTMLElement>) => getComputedStyle(probe.nativeElement).font;
    return {
      surface: token('--mat-sys-surface'),
      onSurface: token('--mat-sys-on-surface'),
      onSurfaceVariant: token('--mat-sys-on-surface-variant'),
      outline: token('--mat-sys-outline-variant'),
      focus: token('--mat-sys-primary'),
      fills: {
        off: pair('--app-load-0', '--app-load-0-on'),
        absent: pair('--mat-sys-surface', '--mat-sys-on-surface-variant'),
        free: pair('--app-load-0', '--app-load-0-on'),
        low: pair('--app-load-1', '--app-load-1-on'),
        medium: pair('--app-load-2', '--app-load-2-on'),
        high: pair('--app-load-3', '--app-load-3-on'),
        full: pair('--app-load-4', '--app-load-4-on'),
        over: pair('--app-load-over', '--app-load-over-on'),
      },
      overOutline: token('--app-load-over-outline'),
      provisional: token('--app-load-provisional-a'),
      hidden: token('--app-load-confidential-a'),
      holiday: token('--mat-sys-error'),
      label: font(this.fontLabel()),
      small: font(this.fontSmall()),
      strong: font(this.fontStrong()),
    };
  }

  private draw(): void {
    const element = this.scroller().nativeElement;
    const canvas = this.canvas().nativeElement;
    const width = element.clientWidth;
    const height = element.clientHeight;
    if (width === 0 || height === 0) return;
    const ratio = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      canvas.style.width = `${width}px`;
      canvas.style.height = `${height}px`;
    }
    const context = canvas.getContext('2d');
    if (!context) return;
    this.palette ??= this.readPalette();
    const palette = this.palette;
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    context.fillStyle = palette.surface;
    context.fillRect(0, 0, width, height);

    const { rowHeight, columnWidth, headerHeight, rowHeaderWidth, gap } = HEATMAP_SIZES;
    const rows = this.rows();
    const columns = this.columns();
    const cells = this.cells();
    const scrollTop = element.scrollTop;
    const scrollLeft = element.scrollLeft;
    const visibleRows = visibleRange(scrollTop, height - headerHeight, rowHeight, rows.length);
    const visibleColumns = visibleRange(scrollLeft, width - rowHeaderWidth, columnWidth, columns.length);
    context.textBaseline = 'middle';

    // The cells in view, under the headers.
    context.save();
    context.beginPath();
    context.rect(rowHeaderWidth, headerHeight, width - rowHeaderWidth, height - headerHeight);
    context.clip();
    for (let r = visibleRows.first; r <= visibleRows.last; r++) {
      for (let c = visibleColumns.first; c <= visibleColumns.last; c++) {
        const cell = cells[r]?.[c];
        if (!cell) continue;
        const x = rowHeaderWidth + c * columnWidth - scrollLeft + gap / 2;
        const y = headerHeight + r * rowHeight - scrollTop + gap / 2;
        this.drawCell(context, palette, cell, x, y, columnWidth - gap, rowHeight - gap);
      }
    }
    // The focused cell.
    const focus = this.focused();
    if (this.focusedInside && document.activeElement === element) {
      const x = rowHeaderWidth + focus.column * columnWidth - scrollLeft;
      const y = headerHeight + focus.row * rowHeight - scrollTop;
      context.strokeStyle = palette.focus;
      context.lineWidth = 3;
      context.strokeRect(x + 1.5, y + 1.5, columnWidth - 3, rowHeight - 3);
    }
    context.restore();

    // The row headers, left.
    context.save();
    context.beginPath();
    context.rect(0, headerHeight, rowHeaderWidth, height - headerHeight);
    context.clip();
    context.fillStyle = palette.surface;
    context.fillRect(0, headerHeight, rowHeaderWidth, height - headerHeight);
    for (let r = visibleRows.first; r <= visibleRows.last; r++) {
      const row = rows[r];
      const y = headerHeight + r * rowHeight - scrollTop;
      context.textAlign = 'left';
      context.fillStyle = palette.onSurface;
      context.font = palette.strong;
      context.fillText(this.fit(context, row.label, rowHeaderWidth - 16), 8, y + rowHeight / 2 - (row.sublabel ? 9 : 0));
      if (row.sublabel) {
        context.fillStyle = palette.onSurfaceVariant;
        context.font = palette.small;
        context.fillText(this.fit(context, row.sublabel, rowHeaderWidth - 16), 8, y + rowHeight / 2 + 10);
      }
    }
    context.restore();

    // The column headers, top.
    context.save();
    context.beginPath();
    context.rect(rowHeaderWidth, 0, width - rowHeaderWidth, headerHeight);
    context.clip();
    context.fillStyle = palette.surface;
    context.fillRect(rowHeaderWidth, 0, width - rowHeaderWidth, headerHeight);
    context.textAlign = 'center';
    for (let c = visibleColumns.first; c <= visibleColumns.last; c++) {
      const column = columns[c];
      const x = rowHeaderWidth + c * columnWidth - scrollLeft + columnWidth / 2;
      context.fillStyle = palette.onSurface;
      context.font = palette.strong;
      context.fillText(this.fit(context, column.label, columnWidth - 4), x, 16);
      context.fillStyle = palette.onSurfaceVariant;
      context.font = palette.small;
      if (column.sublabel) context.fillText(this.fit(context, column.sublabel, columnWidth - 4), x, 34);
      if (column.note) {
        context.fillStyle = palette.holiday;
        context.fillText(this.fit(context, column.note, columnWidth - 4), x, 52);
      }
    }
    context.restore();

    // The corner.
    context.fillStyle = palette.surface;
    context.fillRect(0, 0, rowHeaderWidth, headerHeight);
    context.textAlign = 'left';
    context.fillStyle = palette.onSurfaceVariant;
    context.font = palette.label;
    context.fillText(this.corner(), 8, headerHeight / 2);
    context.strokeStyle = palette.outline;
    context.lineWidth = 1;
    context.beginPath();
    context.moveTo(0, headerHeight - 0.5);
    context.lineTo(width, headerHeight - 0.5);
    context.stroke();
  }

  private drawCell(
    context: CanvasRenderingContext2D,
    palette: Palette,
    cell: HeatmapCellData,
    x: number,
    y: number,
    w: number,
    h: number,
  ): void {
    const radius = 8;
    const shape = () => {
      context.beginPath();
      context.roundRect(x, y, w, h, radius);
    };
    if (cell.level !== 'summary') {
      const colors = palette.fills[cell.level];
      shape();
      context.fillStyle = colors.fill;
      context.fill();
      if (cell.provisional || cell.hidden) {
        context.save();
        shape();
        context.clip();
        context.strokeStyle = cell.hidden ? palette.hidden : palette.provisional;
        context.lineWidth = 4;
        for (let offset = -h; offset < w; offset += 10) {
          context.beginPath();
          context.moveTo(x + offset, y + h);
          context.lineTo(x + offset + h, y);
          context.stroke();
        }
        context.restore();
      }
      if (cell.level === 'over' || cell.level === 'absent') {
        shape();
        context.strokeStyle = cell.level === 'over' ? palette.overOutline : palette.outline;
        context.lineWidth = cell.level === 'over' ? 2 : 1;
        context.stroke();
      }
      context.fillStyle = colors.text;
    } else {
      context.fillStyle = palette.onSurface;
    }
    context.textAlign = 'center';
    context.font = palette.strong;
    context.fillText(this.fit(context, cell.text, w - 4), x + w / 2, y + h / 2 - (cell.subtext ? 8 : 0));
    if (cell.subtext) {
      context.font = palette.small;
      context.fillText(this.fit(context, cell.subtext, w - 4), x + w / 2, y + h / 2 + 10);
    }
  }

  /** `text`, cut with an ellipsis to fit in `width`. */
  private fit(context: CanvasRenderingContext2D, text: string, width: number): string {
    if (context.measureText(text).width <= width) return text;
    let cut = text;
    while (cut.length > 1 && context.measureText(`${cut}…`).width > width) cut = cut.slice(0, -1);
    return `${cut}…`;
  }
}
