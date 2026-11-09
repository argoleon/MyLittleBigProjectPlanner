import { DOCUMENT, Injectable, InjectionToken, inject } from '@angular/core';
import { takeUntilDestroyed } from '@angular/core/rxjs-interop';
import { RouterStateSnapshot, TitleStrategy } from '@angular/router';
import { TranslocoService } from '@jsverse/transloco';
import { Subject, combineLatest, of, switchMap } from 'rxjs';

/** Translation key of the application name, appended to page titles. */
export const APP_NAME_KEY = new InjectionToken<string>('APP_NAME_KEY', {
  factory: () => 'app.name',
});

/**
 * Page titles from translation keys: a route declares `title: 'nav.workload'`
 * and the document title becomes "Workload · My Little Big Project Planner",
 * updated when the language changes.
 */
@Injectable({ providedIn: 'root' })
export class TranslatedTitleStrategy extends TitleStrategy {
  private readonly document = inject(DOCUMENT);
  private readonly transloco = inject(TranslocoService);
  private readonly appNameKey = inject(APP_NAME_KEY);
  private readonly pageKey = new Subject<string | undefined>();

  constructor() {
    super();
    this.pageKey
      .pipe(
        switchMap((key) =>
          combineLatest([
            key ? this.transloco.selectTranslate<string>(key) : of(null),
            this.transloco.selectTranslate<string>(this.appNameKey),
          ]),
        ),
        takeUntilDestroyed(),
      )
      .subscribe(([page, app]) => {
        this.document.title = page ? `${page} · ${app}` : app;
      });
  }

  override updateTitle(snapshot: RouterStateSnapshot): void {
    this.pageKey.next(this.buildTitle(snapshot));
  }
}
