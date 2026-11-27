import { Injectable, inject } from '@angular/core';
import { MatDialog } from '@angular/material/dialog';
import { TranslocoService } from '@jsverse/transloco';
import { firstValueFrom } from 'rxjs';
import {
  Api,
  Assignment,
  ConfirmDialog,
  ConfirmDialogData,
  LocaleService,
  confirmAssignment,
  errorTranslationKey,
  isApiProblem,
} from 'ui';

import { overloadDaysText } from './overload-text';

/** How confirming a provisional assignment ended. */
export type ConfirmationOutcome =
  | { readonly status: 'confirmed'; readonly assignment: Assignment }
  /** The member declined to force the overload: nothing changed. */
  | { readonly status: 'cancelled' }
  | { readonly status: 'refused'; readonly messageKey: string; readonly params?: Record<string, string> };

/**
 * Confirms a provisional assignment (L08-US09): it becomes firm and is
 * checked for overload as a firm assignment. Refused for overload, those
 * who may force are asked whether to; the others are told the days.
 */
@Injectable({ providedIn: 'root' })
export class ProvisionalConfirmation {
  private readonly api = inject(Api);
  private readonly dialog = inject(MatDialog);
  private readonly locale = inject(LocaleService);
  private readonly transloco = inject(TranslocoService);

  /**
   * Confirms the assignment `id` at `version`; `restoreFocus` is where the
   * focus goes back after the dialog asking to force.
   */
  async confirm(id: string, version: number, restoreFocus: string): Promise<ConfirmationOutcome> {
    try {
      return { status: 'confirmed', assignment: await this.send(id, version, false) };
    } catch (error) {
      if (!isApiProblem(error)) return { status: 'refused', messageKey: errorTranslationKey('unknown') };
      if (!error.overload) return { status: 'refused', messageKey: error.translationKey };
      const days = overloadDaysText(error.overload.days, this.locale.formattingLocale(), (key, params) =>
        this.transloco.translate(key, params),
      );
      if (!error.overload.forceable) return { status: 'refused', messageKey: 'assignments.confirm.blocked', params: { days } };
      const force = await firstValueFrom(
        this.dialog
          .open<ConfirmDialog, ConfirmDialogData, boolean>(ConfirmDialog, {
            data: {
              titleKey: 'assignments.confirm.forceTitle',
              messageKey: 'assignments.confirm.forceMessage',
              confirmKey: 'assignments.confirm.force',
              params: { days },
            },
            width: 'min(480px, 100vw)',
            restoreFocus,
          })
          .afterClosed(),
      );
      if (!force) return { status: 'cancelled' };
      try {
        return { status: 'confirmed', assignment: await this.send(id, version, true) };
      } catch (forced) {
        return { status: 'refused', messageKey: isApiProblem(forced) ? forced.translationKey : errorTranslationKey('unknown') };
      }
    }
  }

  private send(id: string, version: number, force: boolean): Promise<Assignment> {
    return this.api.invoke(confirmAssignment, { id, 'If-Match': `"${version}"`, body: { force } });
  }
}
