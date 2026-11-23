import { Component, effect, inject, resource, signal } from '@angular/core';
import { FormControl, FormGroup, ReactiveFormsModule, Validators } from '@angular/forms';
import { MatButtonModule } from '@angular/material/button';
import { MatButtonToggleModule } from '@angular/material/button-toggle';
import { MatFormFieldModule } from '@angular/material/form-field';
import { MatInputModule } from '@angular/material/input';
import { MatProgressBarModule } from '@angular/material/progress-bar';
import { TranslocoPipe } from '@jsverse/transloco';
import {
  Api,
  ErrorState,
  WorkloadSettings,
  errorTranslationKey,
  fieldErrorTranslationKey,
  getWorkloadSettings,
  isApiProblem,
  updateWorkloadSettings,
} from 'ui';

import { WeekHours, hoursText, weekHoursForm, weekHoursValue } from '../capacity/week-hours';

/** Overload modes, as the API names them. */
export type OverloadMode = 'blocking' | 'warning';

/**
 * "Workload" tab of the administration (specification §8, L07-US03): the
 * organization's default capacity, which applies to every resource without
 * a capacity period, its overload tolerance and mode. Tenant admins only.
 */
@Component({
  selector: 'app-workload-settings',
  imports: [
    ErrorState,
    MatButtonModule,
    MatButtonToggleModule,
    MatFormFieldModule,
    MatInputModule,
    MatProgressBarModule,
    ReactiveFormsModule,
    TranslocoPipe,
    WeekHours,
  ],
  templateUrl: './workload-settings.html',
  styleUrl: './workload-settings.scss',
})
export class WorkloadSettingsTab {
  private readonly api = inject(Api);

  protected readonly current = resource({ loader: () => this.api.invoke(getWorkloadSettings) });
  protected readonly form = new FormGroup({
    week: weekHoursForm([420, 420, 420, 420, 420, 0, 0]),
    tolerance: new FormControl(0, { nonNullable: true, validators: [Validators.required, Validators.min(0), Validators.max(100)] }),
    mode: new FormControl<OverloadMode>('blocking', { nonNullable: true }),
  });
  /** The version the form was filled from. */
  private readonly version = signal(0);
  protected readonly saving = signal(false);
  protected readonly notice = signal<{ ok: boolean; messageKey: string } | null>(null);

  constructor() {
    effect(() => {
      if (this.current.hasValue()) this.fill(this.current.value());
    });
  }

  private fill(settings: WorkloadSettings): void {
    this.version.set(settings.version);
    this.form.controls.week.controls.forEach((control, day) => control.setValue(hoursText(settings.defaultCapacityMinutes[day] ?? 0)));
    this.form.controls.tolerance.setValue(settings.overloadTolerancePercent);
    this.form.controls.mode.setValue(settings.overloadMode as OverloadMode);
    this.form.markAsPristine();
  }

  protected async save(): Promise<void> {
    if (this.form.invalid) {
      this.form.markAllAsTouched();
      return;
    }
    this.saving.set(true);
    this.notice.set(null);
    const { tolerance, mode } = this.form.getRawValue();
    try {
      const saved = await this.api.invoke(updateWorkloadSettings, {
        'If-Match': `"${this.version()}"`,
        body: {
          defaultCapacityMinutes: weekHoursValue(this.form.controls.week),
          overloadTolerancePercent: tolerance,
          overloadMode: mode,
        },
      });
      this.fill(saved);
      this.notice.set({ ok: true, messageKey: 'admin.workload.saved' });
    } catch (error) {
      this.notice.set({ ok: false, messageKey: this.place(error) });
    } finally {
      this.saving.set(false);
    }
  }

  /** Field errors on their field; the message of the whole error. */
  private place(error: unknown): string {
    if (!isApiProblem(error)) return errorTranslationKey('unknown');
    for (const fieldError of error.fieldErrors) {
      const day = /^defaultCapacityMinutes\.(\d)$/.exec(fieldError.field);
      const control = day
        ? this.form.controls.week.at(Number(day[1]))
        : fieldError.field === 'overloadTolerancePercent'
          ? this.form.controls.tolerance
          : null;
      control?.setErrors({ api: fieldErrorTranslationKey(fieldError.code) });
      control?.markAsTouched();
    }
    return error.translationKey;
  }
}
