import { moveItemInArray } from '@angular/cdk/drag-drop';
import { AbstractControl, FormArray, FormControl, FormGroup, ValidationErrors, Validators } from '@angular/forms';
import {
  AssignTarget,
  ConfigField,
  FieldKind,
  NamedRef,
  PostFunctionKind,
  TransitionPostFunction,
  TransitionRuleReferences,
  TransitionRules,
  Translations,
  fieldErrorTranslationKey,
  isApiProblem,
} from 'ui';

/** The kinds of post-function, in the order they are offered. */
export const POST_FUNCTION_KINDS: readonly PostFunctionKind[] = ['assign', 'set_field', 'clear_field', 'add_comment'];

/** Whom the Assign post-function may assign to. */
export const ASSIGN_TARGETS: readonly AssignTarget[] = ['current_user', 'reporter', 'member', 'nobody'];

/** Longest comment a post-function adds. */
export const COMMENT_MAX = 2000;

/**
 * How the value of "Set a field" is entered, by the type of the field; the
 * other types (date and time, cascading select, user, group, link to a
 * ticket) can be cleared here, and set through the API.
 */
export type ValueInput = 'text' | 'long_text' | 'number' | 'checkbox' | 'date' | 'select' | 'multi' | 'labels';

const VALUE_INPUTS: Partial<Record<FieldKind, ValueInput>> = {
  short_text: 'text',
  rich_text: 'long_text',
  url: 'text',
  number: 'number',
  checkbox: 'checkbox',
  date: 'date',
  single_select: 'select',
  multi_select: 'multi',
  labels: 'labels',
};

export function valueInput(kind: FieldKind | undefined): ValueInput | null {
  return (kind && VALUE_INPUTS[kind]) ?? null;
}

/** The value as the form holds it, from the API's. */
export function formValue(input: ValueInput | null, value: unknown): unknown {
  if (value === undefined || value === null) return input === 'checkbox' ? false : input === 'multi' ? [] : null;
  if (input === 'labels' && Array.isArray(value)) return value.join(', ');
  return value;
}

/** The value for the API, from the form's; null when nothing is entered. */
export function apiValue(input: ValueInput | null, value: unknown): unknown {
  switch (input) {
    case 'labels':
      return typeof value === 'string'
        ? value
            .split(/[\s,]+/)
            .map((label) => label.trim())
            .filter((label) => label !== '')
        : [];
    case 'number':
      return value === '' || value === null || value === undefined ? null : Number(value);
    case 'checkbox':
      return value === true;
    case 'multi':
      return Array.isArray(value) ? value : [];
    default:
      return typeof value === 'string' && value.trim() === '' ? null : (value ?? null);
  }
}

/** A post-function as the form holds it. */
export interface PostFunctionControls {
  kind: FormControl<PostFunctionKind>;
  assignTo: FormControl<AssignTarget>;
  /** The member, with their name to show. */
  member: FormControl<NamedRef | null>;
  fieldId: FormControl<string | null>;
  value: FormControl<unknown>;
  text: FormControl<string>;
}

export type PostFunctionForm = FormGroup<PostFunctionControls>;

/** The rules as the form holds them; groups with their names to show. */
export interface RulesControls {
  roleIds: FormControl<string[]>;
  groups: FormControl<NamedRef[]>;
  validators: FormControl<string[]>;
  postFunctions: FormArray<PostFunctionForm>;
}

export type RulesForm = FormGroup<RulesControls>;

/** What each kind requires, checked on the post-function as a whole. */
function complete(control: AbstractControl): ValidationErrors | null {
  const post = (control as PostFunctionForm).getRawValue();
  switch (post.kind) {
    case 'assign':
      return post.assignTo === 'member' && !post.member ? { member: true } : null;
    case 'set_field':
    case 'clear_field':
      return post.fieldId ? null : { field: true };
    case 'add_comment':
      return post.text.trim() ? null : { text: true };
  }
}

/** The form of a post-function, as the API returned it or a new one. */
export function postFunctionForm(
  post: TransitionPostFunction | null,
  references: TransitionRuleReferences | null,
  fields: readonly ConfigField[],
): PostFunctionForm {
  const member = post?.accountId
    ? (references?.members.find((m) => m.id === post.accountId) ?? { id: post.accountId, name: '' })
    : null;
  const field = fields.find((f) => f.id === post?.fieldId);
  return new FormGroup<PostFunctionControls>(
    {
      kind: new FormControl(post?.kind ?? 'assign', { nonNullable: true }),
      assignTo: new FormControl(post?.assignTo ?? 'current_user', { nonNullable: true }),
      member: new FormControl(member),
      fieldId: new FormControl(post?.fieldId ?? null),
      value: new FormControl<unknown>(formValue(valueInput(field?.kind), post?.value)),
      text: new FormControl(post?.text ?? '', { nonNullable: true, validators: Validators.maxLength(COMMENT_MAX) }),
    },
    { validators: complete },
  );
}

/** The form of the rules of a transition. */
export function rulesForm(
  rules: TransitionRules | null | undefined,
  references: TransitionRuleReferences | null,
  fields: readonly ConfigField[],
): RulesForm {
  const groups = (rules?.conditions?.groupIds ?? []).map(
    (id) => references?.groups.find((g) => g.id === id) ?? { id, name: '' },
  );
  return new FormGroup<RulesControls>({
    roleIds: new FormControl([...(rules?.conditions?.roleIds ?? [])], { nonNullable: true }),
    groups: new FormControl(groups, { nonNullable: true }),
    validators: new FormControl([...(rules?.validators ?? [])], { nonNullable: true }),
    postFunctions: new FormArray((rules?.postFunctions ?? []).map((post) => postFunctionForm(post, references, fields))),
  });
}

/** The rules for the API; only the properties each kind takes. */
export function rulesValue(form: RulesForm, fields: readonly ConfigField[]): TransitionRules {
  const value = form.getRawValue();
  return {
    conditions: { roleIds: value.roleIds, groupIds: value.groups.map((g) => g.id) },
    validators: value.validators,
    postFunctions: value.postFunctions.map((post): TransitionPostFunction => {
      switch (post.kind) {
        case 'assign':
          return post.assignTo === 'member'
            ? { kind: 'assign', assignTo: 'member', accountId: post.member?.id ?? null }
            : { kind: 'assign', assignTo: post.assignTo };
        case 'set_field': {
          const kind = fields.find((f) => f.id === post.fieldId)?.kind;
          return { kind: 'set_field', fieldId: post.fieldId, value: apiValue(valueInput(kind), post.value) };
        }
        case 'clear_field':
          return { kind: 'clear_field', fieldId: post.fieldId };
        case 'add_comment':
          return { kind: 'add_comment', text: post.text.trim() };
      }
    }),
  };
}

/** Moves the post-function at `from` to `to`: they run in their order. */
export function movePostFunction(posts: FormArray<PostFunctionForm>, from: number, to: number): void {
  if (from === to || to < 0 || to >= posts.length) return;
  const controls = [...posts.controls];
  moveItemInArray(controls, from, to);
  posts.clear({ emitEvent: false });
  for (const control of controls) posts.push(control, { emitEvent: false });
  posts.updateValueAndValidity();
  posts.markAsDirty();
}

/**
 * Puts the errors of the API on the post-functions they name
 * (`rules.postFunctions[1].value`), as `{ api: translationKey }`; true when
 * every error found its place there, so nothing is left to say above.
 */
export function placeRuleErrors(error: unknown, form: RulesForm): boolean {
  if (!isApiProblem(error) || error.fieldErrors.length === 0) return false;
  let everyPlaced = true;
  for (const fieldError of error.fieldErrors) {
    const match = /^rules\.postFunctions\[(\d+)\]\.(\w+)$/.exec(fieldError.field);
    const post = match ? form.controls.postFunctions.at(Number(match[1])) : undefined;
    const name = match ? (({ accountId: 'member' } as Record<string, string>)[match[2]] ?? match[2]) : '';
    const control = post?.get(name);
    if (!control) {
      everyPlaced = false;
      continue;
    }
    control.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
    control.markAsTouched();
  }
  return everyPlaced;
}

/** One line of the summary of a post-function: a translation key and its parameters. */
export interface Summary {
  readonly key: string;
  readonly params: Record<string, string>;
}

/** What the rules of a transition do, in a few words, for the transitions table. */
export function postFunctionSummaries(
  rules: TransitionRules | undefined,
  references: TransitionRuleReferences | null,
  fieldName: (id: string | null | undefined) => string,
): Summary[] {
  return (rules?.postFunctions ?? []).map((post): Summary => {
    switch (post.kind) {
      case 'assign':
        return post.assignTo === 'member'
          ? {
              key: 'config.workflows.rules.summary.assignMember',
              params: { name: references?.members.find((m) => m.id === post.accountId)?.name ?? '' },
            }
          : { key: `config.workflows.rules.summary.assign.${post.assignTo ?? 'nobody'}`, params: {} };
      case 'set_field':
        return { key: 'config.workflows.rules.summary.set', params: { field: fieldName(post.fieldId) } };
      case 'clear_field':
        return { key: 'config.workflows.rules.summary.clear', params: { field: fieldName(post.fieldId) } };
      case 'add_comment':
        return { key: 'config.workflows.rules.summary.comment', params: {} };
    }
  });
}

/** The roles and groups a transition is restricted to, by name; empty: open to everyone. */
export function conditionNames(
  rules: TransitionRules | undefined,
  references: TransitionRuleReferences | null,
  roleLabel: (labels: Translations) => string,
): { roles: (string | null)[]; groups: (string | null)[] } {
  const roles = (rules?.conditions?.roleIds ?? []).map((id) => {
    const role = references?.roles.find((r) => r.id === id);
    return role ? roleLabel(role.labelTranslations) : null;
  });
  const groups = (rules?.conditions?.groupIds ?? []).map((id) => references?.groups.find((g) => g.id === id)?.name ?? null);
  return { roles, groups };
}
