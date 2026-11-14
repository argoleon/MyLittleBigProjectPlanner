import { Component, inject, input, signal } from '@angular/core';
import { ReactiveFormsModule } from '@angular/forms';
import { MatAutocompleteModule, MatAutocompleteSelectedEvent } from '@angular/material/autocomplete';
import { MatButtonModule } from '@angular/material/button';
import { MatChipsModule } from '@angular/material/chips';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatIconModule } from '@angular/material/icon';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { MatSlideToggleModule } from '@angular/material/slide-toggle';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  AccessRole,
  Api,
  ConfigField,
  GrantCandidate,
  HolderKind,
  NamedRef,
  TranslatedPipe,
  listTransitionCandidates,
} from 'ui';

import {
  ASSIGN_TARGETS,
  COMMENT_MAX,
  POST_FUNCTION_KINDS,
  PostFunctionForm,
  RulesForm,
  movePostFunction,
  postFunctionForm,
  valueInput,
} from './transition-rules';

/**
 * The rules of a transition, in its dialog (L04-US05): who may take it
 * (roles, groups found by name), the fields it requires, and its
 * post-functions in their order, each with what its kind takes.
 */
@Component({
  selector: 'app-transition-rules',
  imports: [
    MatAutocompleteModule,
    MatButtonModule,
    MatChipsModule,
    MatFormFieldModule,
    MatIconModule,
    MatInputModule,
    MatSelectModule,
    MatSlideToggleModule,
    ReactiveFormsModule,
    TranslatedPipe,
    TranslocoPipe,
  ],
  styleUrl: './transition-rules-editor.scss',
  templateUrl: './transition-rules-editor.html',
})
export class TransitionRulesEditor {
  private readonly api = inject(Api);

  readonly form = input.required<RulesForm>();
  /** The roles of the tenant. */
  readonly roles = input.required<readonly AccessRole[]>();
  /** The fields of the configuration. */
  readonly fields = input.required<readonly ConfigField[]>();

  protected readonly kinds = POST_FUNCTION_KINDS;
  protected readonly targets = ASSIGN_TARGETS;
  protected readonly commentMax = COMMENT_MAX;
  protected readonly groupOptions = signal<NamedRef[]>([]);
  protected readonly memberOptions = signal<GrantCandidate[]>([]);
  private searches = 0;

  /** The fields "Set a field" can give a value to here. */
  protected settable(): ConfigField[] {
    return this.fields().filter((f) => valueInput(f.kind) !== null);
  }

  protected inputOf(post: PostFunctionForm): ReturnType<typeof valueInput> {
    return valueInput(this.fields().find((f) => f.id === post.controls.fieldId.value)?.kind);
  }

  /** The active values of the list field of `post`. */
  protected valuesOf(post: PostFunctionForm) {
    const field = this.fields().find((f) => f.id === post.controls.fieldId.value);
    return (field?.options.values ?? []).filter((v) => v.active);
  }

  /** The groups or members whose name holds `text`. */
  protected async search(text: string, kind: HolderKind): Promise<void> {
    const search = ++this.searches;
    const found = await this.api.invoke(listTransitionCandidates, { search: text.trim() || undefined, limit: 10 });
    if (search !== this.searches) return;
    const matching = found.filter((c) => c.kind === kind);
    if (kind === 'group') this.groupOptions.set(matching.map((c) => ({ id: c.id, name: c.name })));
    else this.memberOptions.set(matching);
  }

  protected addGroup(event: MatAutocompleteSelectedEvent, input: HTMLInputElement): void {
    const group = event.option.value as NamedRef;
    const groups = this.form().controls.groups;
    if (!groups.value.some((g) => g.id === group.id)) groups.setValue([...groups.value, group]);
    groups.markAsDirty();
    input.value = '';
    this.groupOptions.set([]);
  }

  protected removeGroup(group: NamedRef): void {
    const groups = this.form().controls.groups;
    groups.setValue(groups.value.filter((g) => g.id !== group.id));
    groups.markAsDirty();
  }

  protected chooseMember(post: PostFunctionForm, event: MatAutocompleteSelectedEvent): void {
    const member = event.option.value as GrantCandidate;
    post.controls.member.setValue({ id: member.id, name: member.name });
    post.controls.member.markAsDirty();
  }

  protected memberName = (member: NamedRef | GrantCandidate | string | null): string =>
    typeof member === 'string' ? member : (member?.name ?? '');

  protected add(): void {
    this.form().controls.postFunctions.push(postFunctionForm(null, null, this.fields()));
  }

  protected remove(index: number): void {
    this.form().controls.postFunctions.removeAt(index);
  }

  protected move(from: number, to: number): void {
    movePostFunction(this.form().controls.postFunctions, from, to);
  }

  /** A new field for "Set a field": its value starts again. */
  protected fieldChanged(post: PostFunctionForm): void {
    const input = this.inputOf(post);
    post.controls.value.setValue(input === 'checkbox' ? false : input === 'multi' ? [] : null);
  }
}
