import { Component, computed, effect, inject, resource, signal, untracked } from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatChipsModule } from '@angular/material/chips';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSelectModule } from '@angular/material/select';
import { MatTableModule } from '@angular/material/table';
import { ActivatedRoute, ParamMap, RouterLink } from '@angular/router';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  DurationPipe,
  ErrorState,
  HeatmapGrid,
  HeatmapCellSegment,
  HeatmapPosition,
  HeatmapScale,
  InstantPipe,
  LocalDatePipe,
  LocaleService,
  Node,
  formatDuration,
  formatNumber,
  getWorkloadCell,
  getWorkloadHeatmap,
  listNodes,
  listTeams,
  toLocalDate,
} from 'ui';

import { anchorOf, buildModel, rangeOf, shift } from './heatmap-model';

/** How the heatmap is shown: drawn, or as a table. */
export type HeatmapView = 'heatmap' | 'table';

/** The node types a filter offers, in order. */
const NODE_TYPES = ['portfolio', 'program', 'project'] as const;

/**
 * The workload heatmap (mockup 01, specification §8, L08-US06): the active
 * resources by day, week or month, each cell colored by occupation, of a
 * team or with load on a node's tickets (their whole load shown),
 * provisional load in or out. A cell, clicked or opened with the keyboard,
 * tells its capacity, load and tickets in a side panel; time on tickets the
 * member does not see shows as busy. The table view lists the same data.
 */
@Component({
  selector: 'app-heatmap-page',
  imports: [
    DurationPipe,
    ErrorState,
    HeatmapGrid,
    InstantPipe,
    LocalDatePipe,
    MatButtonModule,
    MatButtonToggleModule,
    MatChipsModule,
    MatFormFieldModule,
    MatIconModule,
    MatProgressBarModule,
    MatSelectModule,
    MatTableModule,
    RouterLink,
    TranslocoPipe,
  ],
  templateUrl: './heatmap-page.html',
  styleUrl: './heatmap-page.scss',
})
export class HeatmapPage {
  private readonly api = inject(Api);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);

  protected readonly scale = signal<HeatmapScale>('week');
  protected readonly anchor = signal(anchorOf('week', toLocalDate(new Date())));
  protected readonly teamId = signal<string | null>(null);
  protected readonly nodeId = signal<string | null>(null);
  protected readonly includeProvisional = signal(true);
  protected readonly view = signal<HeatmapView>('heatmap');
  protected readonly range = computed(() => rangeOf(this.scale(), this.anchor()));

  protected readonly teams = resource({ loader: () => this.api.invoke(listTeams, {}) });
  protected readonly nodes = resource({ loader: () => this.api.invoke(listNodes, { limit: 200 }) });
  /** The nodes the member sees (veiled ones have no name), by type. */
  protected readonly nodeGroups = computed(() => {
    const items: readonly Node[] = this.nodes.hasValue()
      ? this.nodes.value().items.filter((n): n is Node => !('veiled' in n && n.veiled))
      : [];
    return NODE_TYPES.map((type) => ({ type, items: items.filter((n) => n.type === type) })).filter((g) => g.items.length > 0);
  });

  protected readonly data = resource({
    params: () => ({
      scale: this.scale(),
      ...this.range(),
      teamId: this.teamId() ?? undefined,
      nodeId: this.nodeId() ?? undefined,
      includeProvisional: this.includeProvisional(),
    }),
    loader: ({ params }) => this.api.invoke(getWorkloadHeatmap, params),
  });
  protected readonly model = computed(() =>
    this.data.hasValue()
      ? buildModel(this.data.value(), this.locale.formattingLocale(), (key, params) => this.transloco.translate(key, params))
      : null,
  );
  /** The table view's columns: the resource, then each period. */
  protected readonly tableColumns = computed(() => ['resource', ...(this.model()?.columns.map((_, i) => `p${i}`) ?? [])]);
  protected readonly tableRows = computed(() => (this.model()?.rows ?? []).map((row, index) => ({ row, index })));

  /** The cell opened in the side panel. */
  protected readonly selected = signal<HeatmapPosition | null>(null);
  protected readonly selection = computed(() => {
    const at = this.selected();
    const data = this.data.hasValue() ? this.data.value() : null;
    if (!at || !data) return null;
    const row = data.rows[at.row];
    const period = data.periods[at.column];
    return row && period ? { resource: row.resource, period, column: this.model()?.columns[at.column] } : null;
  });
  protected readonly cell = resource({
    params: () => {
      const selection = this.selection();
      return selection
        ? { resourceId: selection.resource.id, from: selection.period.start, to: selection.period.end, nodeId: this.nodeId() ?? undefined }
        : undefined;
    },
    loader: ({ params }) => this.api.invoke(getWorkloadCell, params),
  });

  /** The resource and day a link asked to open, until the heatmap is read. */
  private pending: { resourceId: string; date: string } | null = null;

  constructor() {
    this.follow(inject(ActivatedRoute).snapshot.queryParamMap);
    effect(() => {
      const pending = this.pending;
      if (!pending || !this.data.hasValue()) return;
      const data = this.data.value();
      this.pending = null;
      const row = data.rows.findIndex((r) => r.resource.id === pending.resourceId);
      const column = Math.max(
        0,
        data.periods.findIndex((p) => p.start <= pending.date && pending.date <= p.end),
      );
      if (row >= 0) untracked(() => this.selected.set({ row, column }));
    });
  }

  /**
   * Opens on what a link asks for (the availability search, L08-US10):
   * `scale`, the period of `date`, `teamId`, `provisional=false`, and the
   * details of `resourceId` on `date`.
   */
  private follow(query: ParamMap): void {
    const scale = query.get('scale');
    if (scale === 'day' || scale === 'week' || scale === 'month') this.scale.set(scale);
    const date = query.get('date');
    const day = date && /^\d{4}-\d{2}-\d{2}$/.test(date) ? date : null;
    if (day) this.anchor.set(anchorOf(this.scale(), day));
    const team = query.get('teamId');
    if (team) this.teamId.set(team);
    if (query.get('provisional') === 'false') this.includeProvisional.set(false);
    const resourceId = query.get('resourceId');
    if (resourceId) this.pending = { resourceId, date: day ?? this.range().from };
  }

  protected setScale(scale: HeatmapScale): void {
    this.scale.set(scale);
    this.anchor.set(anchorOf(scale, this.range().from));
    this.selected.set(null);
  }

  protected move(steps: number): void {
    this.anchor.set(shift(this.scale(), this.anchor(), steps));
    this.selected.set(null);
  }

  protected today(): void {
    this.anchor.set(anchorOf(this.scale(), toLocalDate(new Date())));
    this.selected.set(null);
  }

  protected setTeam(id: string | null): void {
    this.teamId.set(id);
    this.selected.set(null);
  }

  protected setNode(id: string | null): void {
    this.nodeId.set(id);
    this.selected.set(null);
  }

  /** A cell opened: the averages row has no details. */
  protected open(at: HeatmapPosition): void {
    const resources = this.data.hasValue() ? this.data.value().rows.length : 0;
    this.selected.set(at.row < resources ? at : null);
  }

  protected close(): void {
    this.selected.set(null);
  }

  /** The load against the capacity: `+4 h`, `−3 h 30`, `0 h`. */
  protected gapText(load: number, capacity: number): string {
    const gap = load - capacity;
    const text = formatDuration(Math.abs(gap), this.locale.formattingLocale());
    return gap > 0 ? `+${text}` : gap < 0 ? `−${text}` : text;
  }

  /** How a segment loads the period: `50 % of the capacity`, `4 h/day`, `Total effort`. */
  protected segmentText(segment: HeatmapCellSegment): string {
    const locale = this.locale.formattingLocale();
    switch (segment.mode) {
      case 'percentage':
        return this.transloco.translate('workload.cell.percentage', { percent: formatNumber(segment.value / 100, locale, { style: 'percent' }) });
      case 'hours_per_day':
        return this.transloco.translate('assignments.perDay', { hours: formatDuration(segment.value, locale) });
      case 'total_effort':
        return this.transloco.translate('assignments.modeNames.total_effort');
    }
  }
}
