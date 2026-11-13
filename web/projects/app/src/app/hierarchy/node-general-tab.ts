import { Component, computed, effect, inject, signal, untracked } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { FormBuilder, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { DateAdapter, provideNativeDateAdapter } from '@angular/material/core';
import { MatDatepickerModule } from '@angular/material/datepicker';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatSelectModule } from '@angular/material/select';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  LocaleService,
  Node,
  NodeOwner,
  errorTranslationKey,
  fieldErrorTranslationKey,
  isApiProblem,
  updateNode,
} from 'ui';

import { HEALTHS, NodeFormValue, changesOf, endsBeforeStart, formValueOf, sameChanges } from './node-form';
import { NodeContext } from './node-settings-page';
import { OwnerPicker } from './owner-picker';

/**
 * "General" tab of a node's settings: name, description, objectives, dates,
 * health and owners, saved with the version they were read at. Read-only
 * for an archived project.
 */
@Component({
  selector: 'app-node-general-tab',
  imports: [
    MatButtonModule,
    MatDatepickerModule,
    MatFormFieldModule,
    MatInputModule,
    MatSelectModule,
    OwnerPicker,
    ReactiveFormsModule,
    TranslocoPipe,
  ],
  // Dates are picked in the user's locale; they travel as local dates.
  providers: [provideNativeDateAdapter()],
  styleUrl: './node-general-tab.scss',
  templateUrl: './node-general-tab.html',
})
export class NodeGeneralTab {
  private readonly api = inject(Api);
  private readonly context = inject(NodeContext);

  protected readonly healths = HEALTHS;
  protected readonly node = this.context.node;
  protected readonly form = inject(FormBuilder).nonNullable.group({
    name: ['', Validators.required],
    description: [''],
    objectives: [''],
    startDate: [null as Date | null],
    endDate: [null as Date | null],
    health: ['on_track' as NodeFormValue['health']],
  });
  protected readonly owners = signal<readonly NodeOwner[]>([]);
  private readonly baseline = signal<NodeFormValue | null>(null);
  private readonly current = signal<NodeFormValue | null>(null);
  /** Whether there are changes not saved yet. */
  readonly dirty = computed(() => {
    const [base, now] = [this.baseline(), this.current()];
    return !!base && !!now && !sameChanges(base, { ...now, owners: this.owners() });
  });
  protected readonly saving = signal(false);
  protected readonly notice = signal<{ ok: boolean; key: string } | null>(null);

  constructor() {
    inject(DateAdapter).setLocale(inject(LocaleService).formattingLocale());
    this.form.valueChanges.pipe(takeUntilDestroyed()).subscribe(() => this.current.set(this.value()));
    // The node loaded, or saved: the form starts from it.
    effect(() => {
      const node = this.node();
      if (node) untracked(() => this.load(node));
    });
    // An archived project is read-only.
    effect(() => {
      if (this.node()?.archived) this.form.disable({ emitEvent: false });
      else this.form.enable({ emitEvent: false });
    });
  }

  protected async save(node: Node): Promise<void> {
    const value = this.value();
    this.form.controls.endDate.setErrors(endsBeforeStart(value.startDate, value.endDate) ? { before_start: true } : null);
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.notice.set(null);
    try {
      const saved = await this.api.invoke(updateNode, {
        id: node.id,
        'If-Match': `"${node.version}"`,
        body: changesOf(value),
      });
      this.context.node.set(saved);
      this.notice.set({ ok: true, key: 'hierarchy.general.saved' });
    } catch (error) {
      this.showError(error);
    } finally {
      this.saving.set(false);
    }
  }

  private value(): NodeFormValue {
    return { ...this.form.getRawValue(), owners: this.owners() };
  }

  private load(node: Node): void {
    const value = formValueOf(node);
    const { owners, ...fields } = value;
    this.form.reset(fields, { emitEvent: false });
    this.owners.set(owners);
    this.baseline.set(value);
    this.current.set(value);
  }

  private showError(error: unknown): void {
    if (!isApiProblem(error)) {
      this.notice.set({ ok: false, key: errorTranslationKey('unknown') });
      return;
    }
    const name = error.fieldErrors.find((e) => e.field === 'name');
    if (name) {
      this.form.controls.name.setErrors({ api: fieldErrorTranslationKey(name.code) });
      this.form.controls.name.markAsTouched();
      return;
    }
    this.notice.set({ ok: false, key: error.translationKey });
  }
}
