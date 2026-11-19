import { ListRange } from '@angular/cdk/collections';
import { CdkVirtualScrollViewport, ScrollingModule } from '@angular/cdk/scrolling';
import {
  Component,
  computed,
  effect,
  inject,
  input,
  resource,
  signal,
  untracked,
  viewChild,
} from '@angular/core';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleGroup, MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatDialog } from '@angular/material/dialog';
import { MatIconModule } from '@angular/material/icon';
import { MatMenuModule } from '@angular/material/menu';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { MatSortModule, Sort } from '@angular/material/sort';
import { MatTableModule } from '@angular/material/table';
import { Router, RouterLink } from '@angular/router';
import { TranslocoPipe, TranslocoService } from '@jsverse/transloco';
import {
  Api,
  EmptyState,
  ErrorState,
  InstantPipe,
  LocalDatePipe,
  Priority,
  SavedFilter,
  SearchResult,
  StatusChip,
  TypeChip,
  errorTranslationKey,
  getSavedFilter,
  isApiProblem,
  parseQuery,
  searchWithQuery,
} from 'ui';

import { chipCategory, chipKind } from '../issues/issue-fields';
import { Criteria, NO_CRITERIA, criteriaOf, queryOf } from './criteria';
import { QueryBuilder } from './query-builder';
import { QueryEditor } from './query-editor';
import { ExportMenu } from '../exports/export-menu';
import { QueryErrorMessage, queryErrorMessage } from './query-error';
import { orderByIndex, sortOf, withSort } from './query-order';
import { SaveFilterData, SaveFilterDialog } from './save-filter-dialog';
import { SavedFilters } from './saved-filters';

/** Results per page. */
const PAGE_SIZE = 50;
/** Height of a row of results, in pixels (the virtual scroll counts on it). */
const ROW_HEIGHT = 52;
/** Load the next page when this few loaded rows are left below the drawn ones. */
const PREFETCH_ROWS = 10;
/** Where the columns chosen by the member are kept, in this browser. */
const COLUMNS_KEY = 'mlbpp.search.columns';
/** Where the last mode the member chose is kept, in this browser. */
const MODE_KEY = 'mlbpp.search.mode';

/** How the query is written: with the visual builder, or as text. */
export type SearchMode = 'builder' | 'query';

/** The mode last chosen in this browser; the builder the first time. */
function storedMode(): SearchMode {
  try {
    return localStorage.getItem(MODE_KEY) === 'query' ? 'query' : 'builder';
  } catch {
    return 'builder';
  }
}

/** A column of the results, and the field of the language that sorts it. */
export interface SearchColumn {
  readonly id: string;
  readonly field: string;
}

/** Every column the results can show, in their order. */
export const SEARCH_COLUMNS: readonly SearchColumn[] = [
  { id: 'key', field: 'key' },
  { id: 'summary', field: 'summary' },
  { id: 'type', field: 'type' },
  { id: 'status', field: 'status' },
  { id: 'assignee', field: 'assignee' },
  { id: 'reporter', field: 'reporter' },
  { id: 'priority', field: 'priority' },
  { id: 'due', field: 'due' },
  { id: 'start', field: 'start' },
  { id: 'project', field: 'project' },
  { id: 'created', field: 'created' },
  { id: 'updated', field: 'updated' },
];

/** The columns shown at first, as mockup 15. */
export const DEFAULT_COLUMNS: readonly string[] = ['key', 'summary', 'type', 'status', 'assignee', 'priority', 'due'];

/** The columns kept in this browser, else the default ones; the key always comes. */
function storedColumns(): readonly string[] {
  try {
    const stored = JSON.parse(localStorage.getItem(COLUMNS_KEY) ?? 'null') as unknown;
    if (Array.isArray(stored)) {
      const known = SEARCH_COLUMNS.map((c) => c.id).filter((id) => id === 'key' || stored.includes(id));
      if (known.length > 1) return known;
    }
  } catch {
    // Storage unavailable or unreadable: the default columns.
  }
  return DEFAULT_COLUMNS;
}

/**
 * Search (mockup 15, L06-US01): a query of the language, run on Enter or
 * with the button, the results in a virtualized table (the CDK table draws
 * only the rows in view of its `cdk-virtual-scroll-viewport`; the next page
 * is loaded on the way), the columns of the
 * member's choice, sorting a column sorting the query. The query is in the
 * address (`?q=`) so it can be shared and comes back with the history.
 *
 * Saved filters (L06-US04): the panel lists the member's and those shared
 * with them; `?filter=` opens one (its query runs, unless `?q=` says
 * otherwise: the member changed it). "Save the filter" keeps the query run.
 */
@Component({
  selector: 'app-search-page',
  imports: [
    EmptyState,
    ErrorState,
    ExportMenu,
    InstantPipe,
    LocalDatePipe,
    MatButtonModule,
    MatButtonToggleModule,
    MatIconModule,
    MatMenuModule,
    MatProgressBarModule,
    MatSortModule,
    MatTableModule,
    Priority,
    QueryBuilder,
    QueryEditor,
    RouterLink,
    SavedFilters,
    ScrollingModule,
    StatusChip,
    TranslocoPipe,
    TypeChip,
  ],
  templateUrl: './search-page.html',
  styleUrl: './search-page.scss',
})
export class SearchPage {
  /** The query of the address (`?q=`). */
  readonly q = input<string>();
  /** The saved filter of the address (`?filter=`). */
  readonly filter = input<string>();

  private readonly api = inject(Api);
  private readonly router = inject(Router);
  private readonly transloco = inject(TranslocoService);
  private readonly editor = viewChild(QueryEditor);
  private readonly modes = viewChild(MatButtonToggleGroup);
  private readonly viewport = viewChild(CdkVirtualScrollViewport);
  private readonly savedFilters = viewChild(SavedFilters);
  private readonly dialog = inject(MatDialog);

  protected readonly rowHeight = ROW_HEIGHT;

  protected readonly allColumns = SEARCH_COLUMNS;
  protected readonly chipCategory = chipCategory;
  protected readonly chipKind = chipKind;
  /** The query being written, in the editor or by the builder. */
  protected readonly draft = signal('');
  protected readonly mode = signal<SearchMode>(storedMode());
  protected readonly criteria = signal<Criteria>(NO_CRITERIA);
  /** Said when a query cannot be shown in the builder. */
  protected readonly notice = signal<string | null>(null);

  /** The saved filter open, if any; bumped when saved again. */
  private readonly filterRead = signal(0);
  protected readonly openFilter = resource({
    params: () => {
      const id = this.filter();
      return id ? { id, read: this.filterRead() } : undefined;
    },
    loader: ({ params }) => this.api.invoke(getSavedFilter, { id: params.id }),
  });
  protected readonly active = computed<SavedFilter | null>(() =>
    this.filter() && this.openFilter.hasValue() ? this.openFilter.value() : null,
  );
  /** The query run: the address's; else the open filter's. */
  protected readonly ran = computed(() => (this.q() ?? this.active()?.query ?? '').trim());
  /** The query run is no longer the open filter's. */
  protected readonly changed = computed(() => {
    const active = this.active();
    return !!active && this.ran() !== active.query.trim();
  });
  /** Bumped to run the same query again. */
  private readonly runs = signal(0);

  protected readonly first = resource({
    // Nothing runs while the filter of the address is being read.
    params: () =>
      this.filter() !== undefined && this.q() === undefined && this.openFilter.isLoading()
        ? undefined
        : { query: this.ran(), run: this.runs() },
    loader: ({ params }) =>
      this.api.invoke(searchWithQuery, { body: { query: params.query, limit: PAGE_SIZE } }),
  });
  /** Pages loaded after the first one, and the cursor of the next. */
  private readonly more = signal<readonly SearchResult[]>([]);
  private readonly moreCursor = signal<string | null | undefined>(undefined);
  protected readonly loadingMore = signal(false);
  protected readonly moreError = signal<string | null>(null);

  protected readonly rows = computed(() => (this.first.hasValue() ? [...this.first.value().items, ...this.more()] : []));
  protected readonly total = computed(() => (this.first.hasValue() ? this.first.value().total : null));
  protected readonly nextCursor = computed(() => {
    const more = this.moreCursor();
    if (more !== undefined) return more;
    return this.first.hasValue() ? this.first.value().nextCursor : null;
  });

  /** The rows the virtual scroll draws. */
  private readonly range = signal<ListRange>({ start: 0, end: 0 });
  protected readonly trackRow = (_: number, row: SearchResult) => row.id;

  /** Why the query was refused, when the API says so. */
  protected readonly queryError = computed<QueryErrorMessage | null>(() => {
    const error = this.first.error();
    if (!isApiProblem(error) || error.code !== 'query.invalid' || !error.query) return null;
    return queryErrorMessage(error.query, (key, params) => this.transloco.translate(key, params));
  });
  /** Another failure than a refused query. */
  protected readonly loadError = computed(() => (this.queryError() ? null : this.first.error()));

  protected readonly columns = signal<readonly string[]>(storedColumns());
  /** The column the query sorts by, if one of them. */
  protected readonly sort = computed(() => {
    const sort = sortOf(this.ran());
    const column = sort && SEARCH_COLUMNS.find((c) => c.field.toLowerCase() === sort.field.toLowerCase());
    return column ? { active: column.id, direction: sort.descending ? ('desc' as const) : ('asc' as const) } : null;
  });

  constructor() {
    // The address's query goes in the field; a new query starts from its first page.
    effect(() => {
      const query = this.ran();
      this.runs();
      untracked(() => {
        this.draft.set(query);
        if (this.mode() === 'builder') void this.showInBuilder(query);
        this.more.set([]);
        this.moreCursor.set(undefined);
        this.moreError.set(null);
        this.range.set({ start: 0, end: 0 });
      });
    });
    // The rows drawn follow the scroll.
    effect((onCleanup) => {
      const viewport = this.viewport();
      if (!viewport) return;
      const subscription = viewport.renderedRangeStream.subscribe((range) => this.range.set(range));
      onCleanup(() => subscription.unsubscribe());
    });
    // A refused query: the editor, the part concerned selected.
    effect(() => {
      const error = this.queryError();
      if (!error) return;
      untracked(() => this.mode.set('query'));
      const editor = this.editor();
      if (editor) editor.focusRange(error.start, Math.max(error.start, error.end));
    });
    // Close to the end of what is loaded: the next page.
    effect(() => {
      const range = this.range();
      if (range.end + PREFETCH_ROWS >= this.rows().length && this.nextCursor() && !this.loadingMore() && !this.moreError()) {
        untracked(() => void this.loadMore());
      }
    });
  }

  /** Runs the query written: in the address, so the history keeps it. */
  protected run(): void {
    const query = this.draft().trim();
    if (query === this.ran()) {
      this.runs.update((n) => n + 1);
      return;
    }
    void this.router.navigate([], { queryParams: { q: query || null }, queryParamsHandling: 'merge' });
  }

  /** The builder changed: its query runs, sorted as before. */
  protected built(criteria: Criteria): void {
    this.criteria.set(criteria);
    const at = orderByIndex(this.ran());
    this.draft.set(queryOf(criteria, at < 0 ? '' : this.ran().slice(at)));
    this.run();
  }

  /** Switches the mode; the builder only for a query it can show. */
  protected async setMode(mode: SearchMode): Promise<void> {
    this.notice.set(null);
    if (mode === 'builder' && !(await this.showInBuilder(this.draft()))) {
      // The toggle the member pressed goes back to the editor.
      const modes = this.modes();
      if (modes) modes.value = 'query';
      return;
    }
    this.mode.set(mode);
    try {
      localStorage.setItem(MODE_KEY, mode);
    } catch {
      // Not kept in this browser.
    }
  }

  /**
   * Shows `query` in the builder when it can; otherwise the editor, with a
   * word why. Whether it could.
   */
  private async showInBuilder(query: string): Promise<boolean> {
    let criteria: Criteria | null = null;
    try {
      criteria = criteriaOf(await this.api.invoke(parseQuery, { body: { query } }));
    } catch {
      // A wrong query: the editor shows why.
    }
    if (criteria) {
      this.criteria.set(criteria);
      this.mode.set('builder');
      return true;
    }
    this.mode.set('query');
    this.notice.set('search.builder.cannot');
    return false;
  }

  /** Sorting a column sorts the query by its field. */
  protected sortBy(sort: Sort): void {
    const column = SEARCH_COLUMNS.find((c) => c.id === sort.active);
    if (!column) return;
    this.draft.set(withSort(this.draft() || this.ran(), column.field, sort.direction));
    this.run();
  }

  protected shows(column: string): boolean {
    return this.columns().includes(column);
  }

  /** Shows or hides `column`; the key always stays. */
  protected toggle(column: string): void {
    if (column === 'key') return;
    const shown = new Set(this.columns());
    if (shown.has(column)) shown.delete(column);
    else shown.add(column);
    const columns = SEARCH_COLUMNS.map((c) => c.id).filter((id) => shown.has(id));
    this.columns.set(columns);
    try {
      localStorage.setItem(COLUMNS_KEY, JSON.stringify(columns));
    } catch {
      // Not kept in this browser: shown for now only.
    }
  }

  protected retry(): void {
    this.runs.update((n) => n + 1);
  }

  /** Saves the query run: into the open filter when it is the member's, else as a new one. */
  protected saveFilter(): void {
    const active = this.active();
    this.openSaveDialog({ query: this.ran(), filter: active?.canEdit ? active : undefined });
  }

  /** Changes one of the member's filters, from the panel. */
  protected editFilter(filter: SavedFilter): void {
    this.openSaveDialog({ query: filter.id === this.active()?.id ? this.ran() : filter.query, filter });
  }

  private openSaveDialog(data: SaveFilterData): void {
    this.dialog
      .open<SaveFilterDialog, SaveFilterData, SavedFilter>(SaveFilterDialog, { data })
      .afterClosed()
      .subscribe((saved) => {
        if (!saved) return;
        this.notice.set('search.filters.saved');
        this.savedFilters()?.reload();
        this.filterRead.update((n) => n + 1);
        // The filter saved opens, its query as saved.
        void this.router.navigate([], { queryParams: { filter: saved.id, q: null }, queryParamsHandling: 'merge' });
      });
  }

  /** A filter was deleted: the open one closes, its results stay. */
  protected filterDeleted(filter: SavedFilter): void {
    this.notice.set('search.filters.deleted');
    if (filter.id === this.filter()) {
      void this.router.navigate([], { queryParams: { filter: null, q: this.ran() || null }, queryParamsHandling: 'merge' });
    }
  }

  protected async loadMore(): Promise<void> {
    const cursor = this.nextCursor();
    if (!cursor) return;
    this.loadingMore.set(true);
    this.moreError.set(null);
    try {
      const page = await this.api.invoke(searchWithQuery, {
        body: { query: this.ran(), limit: PAGE_SIZE, cursor },
      });
      this.more.update((more) => [...more, ...page.items]);
      this.moreCursor.set(page.nextCursor ?? null);
    } catch (error) {
      this.moreError.set(isApiProblem(error) ? error.translationKey : errorTranslationKey('unknown'));
    } finally {
      this.loadingMore.set(false);
    }
  }

  /** The query text before, inside and after the part an error is about. */
  protected excerpt(error: QueryErrorMessage): [string, string, string] {
    const query = this.ran();
    return [query.slice(0, error.start), query.slice(error.start, error.end) || ' ', query.slice(error.end)];
  }
}
