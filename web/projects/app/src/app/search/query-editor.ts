import { TextFieldModule } from '@angular/cdk/text-field';
import { Component, DestroyRef, ElementRef, effect, inject, input, model, output, signal, untracked, viewChild } from '@angular/core';
import { FormControl, ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent, MatAutocompleteTrigger } from '@angular/material/autocomplete';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { TranslocoPipe } from '@jsverse/transloco';
import { Api, Suggestion, suggestQuery } from 'ui';

/** Wait after a keystroke before asking for suggestions, in milliseconds. */
const SUGGEST_DELAY = 150;

/** A suggestion, with the query and the cursor it leads to once chosen. */
export interface SuggestionOption {
  readonly suggestion: Suggestion;
  readonly query: string;
  readonly caret: number;
}

/**
 * Where choosing `suggestion` leads in `query`: its text in place of
 * `from`..`to`, then a space unless one follows or the cursor stays inside
 * (a function's parentheses).
 */
export function optionOf(query: string, from: number, to: number, suggestion: Suggestion): SuggestionOption {
  const inside = suggestion.caret != null;
  const space = !inside && query.charAt(to) !== ' ' ? ' ' : '';
  return {
    suggestion,
    query: query.slice(0, from) + suggestion.text + space + query.slice(to),
    // After the space, added or already there; else where the API says.
    caret: from + (suggestion.caret ?? suggestion.text.length + 1),
  };
}

/**
 * The query editor (L06-US02): an outlined field whose suggestions follow
 * the cursor (`POST /search/suggestions`), with the combobox pattern of
 * `mat-autocomplete` (arrows to browse, Enter to accept, Escape to close;
 * Ctrl+Space opens them). Enter without a suggestion chosen runs the query.
 */
@Component({
  selector: 'app-query-editor',
  imports: [MatAutocompleteModule, MatFormFieldModule, MatInputModule, ReactiveFormsModule, TextFieldModule, TranslocoPipe],
  templateUrl: './query-editor.html',
  styleUrl: './query-editor.scss',
})
export class QueryEditor {
  /** The query, both ways. */
  readonly value = model('');
  /** Ids of what describes the field (a refused query). */
  readonly describedBy = input<string | null>(null);
  readonly invalid = input(false);
  /** Enter (no suggestion chosen) or Ctrl+Enter: run the query. */
  readonly submitted = output<void>();

  private readonly api = inject(Api);
  private readonly textarea = viewChild.required<ElementRef<HTMLTextAreaElement>>('textarea');
  private readonly trigger = viewChild.required(MatAutocompleteTrigger);

  protected readonly control = new FormControl<string | SuggestionOption>('', { nonNullable: true });
  protected readonly options = signal<readonly SuggestionOption[]>([]);
  /** What the field shows for a chosen option: the query it leads to. */
  protected readonly display = (value: string | SuggestionOption | null) =>
    typeof value === 'string' || value === null ? (value ?? '') : value.query;

  private timer: ReturnType<typeof setTimeout> | undefined;
  /** Answers to an older request are dropped. */
  private asked = 0;

  constructor() {
    // The value given from outside goes in the field.
    effect(() => {
      const value = this.value();
      untracked(() => {
        if (this.text() !== value) this.control.setValue(value, { emitEvent: false });
      });
    });
    this.control.valueChanges.subscribe(() => {
      this.value.set(this.text());
      this.schedule();
    });
    inject(DestroyRef).onDestroy(() => clearTimeout(this.timer));
  }

  /** Focuses the field and selects `start`..`end`. */
  focusRange(start: number, end: number): void {
    const textarea = this.textarea().nativeElement;
    textarea.focus();
    textarea.setSelectionRange(start, end);
  }

  private text(): string {
    return this.display(this.control.value);
  }

  /** Suggestions for where the cursor now stands, after a short wait. */
  protected schedule(delay = SUGGEST_DELAY): void {
    clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.suggest(), delay);
  }

  private async suggest(): Promise<void> {
    const textarea = this.textarea().nativeElement;
    const query = this.text();
    const cursor = textarea.selectionStart ?? query.length;
    const asked = ++this.asked;
    try {
      const found = await this.api.invoke(suggestQuery, { body: { query, cursor } });
      if (asked !== this.asked) return;
      this.options.set(found.suggestions.map((s) => optionOf(query, found.from, found.to, s)));
    } catch {
      if (asked === this.asked) this.options.set([]);
    }
    const trigger = this.trigger();
    if (this.options().length > 0 && textarea.ownerDocument.activeElement === textarea) trigger.openPanel();
    else trigger.closePanel();
  }

  protected chosen(event: MatAutocompleteSelectedEvent): void {
    const option = event.option.value as SuggestionOption;
    this.control.setValue(option.query);
    this.focusRange(option.caret, option.caret);
    // On to what may follow.
    this.schedule(0);
  }

  protected onKeydown(event: KeyboardEvent): void {
    if (event.key === ' ' && event.ctrlKey) {
      event.preventDefault();
      this.schedule(0);
      return;
    }
    if (event.key !== 'Enter' || event.shiftKey) return;
    const trigger = this.trigger();
    // Enter on a highlighted suggestion accepts it (the autocomplete does).
    if (event.defaultPrevented || (trigger.panelOpen && trigger.activeOption && !event.ctrlKey)) return;
    event.preventDefault();
    trigger.closePanel();
    clearTimeout(this.timer);
    this.submitted.emit();
  }
}
