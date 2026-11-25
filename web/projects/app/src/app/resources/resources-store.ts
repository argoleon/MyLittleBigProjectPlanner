import { Injectable, computed, inject, resource, signal } from '@angular/core';
import { Api, Resource, ResourceKind, listResources } from 'ui';

/** A status filter chip of the list. */
export type ResourceState = 'active' | 'inactive';

/** What the list asks the API for. */
export interface ResourceQuery {
  readonly search: string | undefined;
  /** `undefined`: both kinds. */
  readonly kind: ResourceKind | undefined;
  /** `undefined`: active and inactive ones. */
  readonly active: boolean | undefined;
  /** Members of this team only. */
  readonly team: string | undefined;
  /** No kind or no status selected: nothing to list. */
  readonly none: boolean;
}

/** The query of the list for `search` and the selected chips. */
export function resourceQuery(
  search: string,
  kinds: readonly ResourceKind[],
  states: readonly ResourceState[],
  team: string | null = null,
): ResourceQuery {
  return {
    search: search.trim() || undefined,
    team: team ?? undefined,
    kind: kinds.length === 1 ? kinds[0] : undefined,
    active: states.length === 1 ? states[0] === 'active' : undefined,
    none: kinds.length === 0 || states.length === 0,
  };
}

/** Resources are listed by pages of this size. */
export const RESOURCE_PAGE_SIZE = 50;

/**
 * State of the resource list (L07-US01): the search and the filter chips,
 * the pages loaded so far, whether the member manages resources. Provided
 * by the list page.
 */
@Injectable()
export class ResourcesStore {
  private readonly api = inject(Api);

  readonly search = signal('');
  readonly kinds = signal<readonly ResourceKind[]>(['person', 'generic']);
  /** Active ones only, at first. */
  readonly states = signal<readonly ResourceState[]>(['active']);
  /** The team filtered by, if any. */
  readonly team = signal<string | null>(null);
  /** Lists a team's members only: nothing until `team` is known (a team page). */
  readonly teamOnly = signal(false);
  readonly query = computed(() => {
    const query = resourceQuery(this.search(), this.kinds(), this.states(), this.team());
    return this.teamOnly() && !query.team ? { ...query, none: true } : query;
  });

  private readonly first = resource({
    params: () => this.query(),
    loader: async ({ params }) => {
      // A new query starts from its first page.
      this.more.set([]);
      this.moreCursor.set(undefined);
      if (params.none) return { items: [] as Resource[], nextCursor: null as string | null };
      const page = await this.api.invoke(listResources, {
        search: params.search,
        kind: params.kind,
        active: params.active,
        team: params.team,
        limit: RESOURCE_PAGE_SIZE,
      });
      this.manager.set(page.canManage);
      return page;
    },
  });
  /** Pages loaded after the first one, and the cursor of the next. */
  private readonly more = signal<readonly Resource[]>([]);
  private readonly moreCursor = signal<string | null | undefined>(undefined);
  private readonly manager = signal(false);
  readonly loadingMore = signal(false);
  /** Why the next page could not be loaded. */
  readonly moreError = signal<unknown>(null);

  readonly items = computed<readonly Resource[]>(() =>
    this.first.hasValue() ? [...this.first.value().items, ...this.more()] : [],
  );
  readonly nextCursor = computed(() => {
    const more = this.moreCursor();
    if (more !== undefined) return more;
    return this.first.hasValue() ? this.first.value().nextCursor : null;
  });
  readonly loading = computed(() => this.first.isLoading());
  readonly error = computed(() => this.first.error());
  /** The member creates and changes resources; known once a page came. */
  readonly canManage = this.manager.asReadonly();
  /**
   * The search or the chips narrow the list beyond its default (active
   * resources of both kinds): an empty list then says nothing matches,
   * rather than that there is no resource yet.
   */
  readonly filtered = computed(
    () =>
      this.query().search !== undefined ||
      this.query().team !== undefined ||
      this.kinds().length < 2 ||
      !this.states().includes('active'),
  );

  setKinds(kinds: readonly ResourceKind[] | ResourceKind | null): void {
    this.kinds.set(asList(kinds));
  }

  setStates(states: readonly ResourceState[] | ResourceState | null): void {
    this.states.set(asList(states));
  }

  reload(): void {
    this.first.reload();
  }

  async loadMore(): Promise<void> {
    const cursor = this.nextCursor();
    const query = this.query();
    if (!cursor || query.none || this.loadingMore()) return;
    this.loadingMore.set(true);
    this.moreError.set(null);
    try {
      const page = await this.api.invoke(listResources, {
        search: query.search,
        kind: query.kind,
        active: query.active,
        team: query.team,
        limit: RESOURCE_PAGE_SIZE,
        cursor,
      });
      this.more.update((more) => [...more, ...page.items]);
      this.moreCursor.set(page.nextCursor ?? null);
    } catch (error) {
      this.moreError.set(error);
    } finally {
      this.loadingMore.set(false);
    }
  }
}

function asList<T>(value: readonly T[] | T | null): readonly T[] {
  return Array.isArray(value) ? value : value ? [value as T] : [];
}
