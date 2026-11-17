import { Component, DestroyRef, computed, effect, inject, model, signal, untracked } from '@angular/core';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatChipsModule } from '@angular/material/chips';
import { DateAdapter, provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, ISSUE_PRIORITY, LocaleService, Suggestion, fromLocalDate, suggestQuery, toLocalDate } from 'ui';

import { CURRENT_USER, CreatedPreset, Criteria, DateRange, DuePreset, NO_CRITERIA } from './criteria';

/** Wait after a keystroke in the text criterion, in milliseconds. */
const TEXT_DELAY = 400;

/** A value of a list criterion: its key, and what it reads as. */
interface Choice {
  readonly key: string;
  readonly label: string;
}

type ListCriterion = 'projects' | 'types' | 'statuses' | 'assignees' | 'priorities';
type DateChoice<P extends string> = P | 'any' | 'custom';

/** The values of `field` the API suggests for `typed`. */
async function choicesOf(api: Api, field: string, typed = ''): Promise<Choice[]> {
  const query = `${field} IN (${typed}`;
  const found = await api.invoke(suggestQuery, { body: { query, cursor: query.length } });
  return found.suggestions
    .filter((s: Suggestion) => s.kind === 'value')
    .map((s: Suggestion) => ({ key: s.text.replace(/^"|"$/g, ''), label: s.label ?? s.text }));
}

/**
 * The visual query builder (L06-US02): a row of criteria (projects, types,
 * statuses, assignees, priorities, due and creation dates, text) the
 * search screen turns into a query (`queryOf`). Values come from the
 * suggestions of the API: the projects the member sees, the members of
 * the organization, the keys of types and statuses with their labels.
 */
@Component({
  selector: 'app-query-builder',
  imports: [
    MatAutocompleteModule,
    MatChipsModule,
    MatDatepickerModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatSelectModule,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  providers: [provideNativeDateAdapter()],
  templateUrl: './query-builder.html',
  styleUrl: './query-builder.scss',
})
export class QueryBuilder {
  readonly criteria = model<Criteria>(NO_CRITERIA);

  private readonly api = inject(Api);

  protected readonly currentUser = CURRENT_USER;
  protected readonly priorities = ISSUE_PRIORITY;
  protected readonly duePresets: readonly DuePreset[] = ['overdue', 'this_week', 'this_month', 'none'];
  protected readonly createdPresets: readonly CreatedPreset[] = ['last_7_days', 'last_30_days'];

  protected readonly types = signal<readonly Choice[]>([]);
  protected readonly statuses = signal<readonly Choice[]>([]);
  protected readonly projectChoices = signal<readonly Choice[]>([]);
  protected readonly assigneeChoices = signal<readonly Choice[]>([]);
  /** What the chips of projects and assignees read as (their keys otherwise). */
  private readonly labels = signal<ReadonlyMap<string, string>>(new Map());

  protected readonly dueChoice = computed<DateChoice<DuePreset>>(() => this.choiceOf(this.criteria().due));
  protected readonly createdChoice = computed<DateChoice<CreatedPreset>>(() => this.choiceOf(this.criteria().created));
  protected readonly dueRange = computed(() => this.rangeOf(this.criteria().due));
  protected readonly createdRange = computed(() => this.rangeOf(this.criteria().created));
  protected readonly text = new FormControl('', { nonNullable: true });

  private textTimer: ReturnType<typeof setTimeout> | undefined;

  constructor() {
    inject(DateAdapter).setLocale(inject(LocaleService).formattingLocale());
    void this.loadLists();
    effect(() => {
      const text = this.criteria().text;
      untracked(() => {
        if (this.text.value !== text) this.text.setValue(text, { emitEvent: false });
      });
    });
    this.text.valueChanges.subscribe((text) => {
      clearTimeout(this.textTimer);
      this.textTimer = setTimeout(() => this.set({ text }), TEXT_DELAY);
    });
    inject(DestroyRef).onDestroy(() => clearTimeout(this.textTimer));
  }

  private async loadLists(): Promise<void> {
    try {
      const [types, statuses] = await Promise.all([choicesOf(this.api, 'type'), choicesOf(this.api, 'status')]);
      this.types.set(types);
      this.statuses.set(statuses);
    } catch {
      // The lists stay empty; the editor still works.
    }
  }

  private set(changes: Partial<Criteria>): void {
    this.criteria.update((criteria) => ({ ...criteria, ...changes }));
  }

  private choiceOf<P extends string>(value: P | DateRange | null): DateChoice<P> {
    if (value === null) return 'any';
    return typeof value === 'string' ? value : 'custom';
  }

  private rangeOf(value: string | DateRange | null): { from: Date | null; to: Date | null } {
    if (value === null || typeof value === 'string') return { from: null, to: null };
    return { from: fromLocalDate(value.from), to: fromLocalDate(value.to) };
  }

  protected label(key: string): string {
    return this.labels().get(key) ?? key;
  }

  protected setList(criterion: 'types' | 'statuses' | 'priorities', values: string[]): void {
    this.set({ [criterion]: values });
  }

  protected setDue(choice: DateChoice<DuePreset>): void {
    this.set({ due: choice === 'any' ? null : choice === 'custom' ? { from: null, to: null } : choice });
  }

  protected setCreated(choice: DateChoice<CreatedPreset>): void {
    this.set({ created: choice === 'any' ? null : choice === 'custom' ? { from: null, to: null } : choice });
  }

  protected setRange(criterion: 'due' | 'created', bound: 'from' | 'to', date: Date | null): void {
    const current = this.criteria()[criterion];
    const range: DateRange = typeof current === 'object' && current !== null ? current : { from: null, to: null };
    this.set({ [criterion]: { ...range, [bound]: date ? toLocalDate(date) : null } });
  }

  /** Projects or members matching what is typed in their chips' input. */
  protected async search(criterion: 'projects' | 'assignees', typed: string): Promise<void> {
    const field = criterion === 'projects' ? 'project' : 'assignee';
    try {
      const found = await choicesOf(this.api, field, typed.trim());
      (criterion === 'projects' ? this.projectChoices : this.assigneeChoices).set(found);
    } catch {
      (criterion === 'projects' ? this.projectChoices : this.assigneeChoices).set([]);
    }
  }

  protected add(criterion: ListCriterion, event: MatAutocompleteSelectedEvent, input: HTMLInputElement): void {
    const choice = event.option.value as Choice;
    const values = this.criteria()[criterion];
    if (!values.includes(choice.key)) this.set({ [criterion]: [...values, choice.key] });
    this.labels.update((labels) => new Map(labels).set(choice.key, choice.label));
    input.value = '';
  }

  protected remove(criterion: ListCriterion, key: string): void {
    this.set({ [criterion]: this.criteria()[criterion].filter((value) => value !== key) });
  }
}
