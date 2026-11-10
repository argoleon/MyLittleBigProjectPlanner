import {
  HttpErrorResponse,
  HttpInterceptorFn,
  provideHttpClient,
  withFetch,
  withInterceptors,
  withXsrfConfiguration,
} from '@angular/common/http';
import { EnvironmentProviders, Injector, inject, makeEnvironmentProviders } from '@angular/core';
import { catchError, throwError } from 'rxjs';

import { AuthService } from '../auth/auth.service';
import { ApiProblem } from './api-problem';
import { ApiConfiguration, provideApiConfiguration } from './generated/api-configuration';
import { getSession } from './generated/fn/auth/get-session';

/** Cookie holding the session's CSRF token, set by the API at sign-in. */
export const CSRF_COOKIE = 'mlbpp_csrf';

/** Header the API expects the CSRF token in, on state-changing requests. */
export const CSRF_HEADER = 'X-CSRF-Token';

/** Options of {@link provideApi}. */
export interface ApiOptions {
  /**
   * Origin of the API, without a trailing slash. Empty (the default) when
   * the application is served by the same origin as the API: paths of the
   * contract already start with `/api/v1`.
   */
  rootUrl?: string;
}

/**
 * Sets up the generated API client: `HttpClient` (fetch backend), the root
 * URL, session cookies, the CSRF token on state-changing requests (copied
 * from the {@link CSRF_COOKIE} cookie by Angular's XSRF support, for
 * same-origin requests), errors turned into {@link ApiProblem}, and the
 * sign-in page shown when the session ends.
 *
 * Applications call it once, instead of `provideHttpClient()`:
 *
 * ```ts
 * bootstrapApplication(App, { providers: [provideApi()] });
 * ```
 *
 * Components then call the API through the generated `Api` service, never
 * through `HttpClient`:
 *
 * ```ts
 * const api = inject(Api);
 * const page = await api.invoke(listReferenceItems, { limit: 20 });
 * ```
 */
export function provideApi(options: ApiOptions = {}): EnvironmentProviders {
  const rootUrl = (options.rootUrl ?? '').replace(/\/+$/, '');
  return makeEnvironmentProviders([
    provideApiConfiguration(rootUrl),
    provideHttpClient(
      withFetch(),
      withXsrfConfiguration({ cookieName: CSRF_COOKIE, headerName: CSRF_HEADER }),
      withInterceptors([apiInterceptor]),
    ),
  ]);
}

/**
 * For requests to the API only: sends the session cookie, turns error
 * responses into {@link ApiProblem}, and tells {@link AuthService} when the
 * API says the session ended (401 `unauthenticated`). Other requests
 * (translation files, assets) are left untouched.
 */
export const apiInterceptor: HttpInterceptorFn = (request, next) => {
  const rootUrl = inject(ApiConfiguration).rootUrl;
  const injector = inject(Injector);
  if (!request.url.startsWith(`${rootUrl}/api/`)) {
    return next(request);
  }
  // Asking for the session is how AuthService learns there is none.
  const sessionCheck = request.url === rootUrl + getSession.PATH;
  return next(request.clone({ withCredentials: true })).pipe(
    catchError((error: unknown) => {
      const problem = error instanceof HttpErrorResponse ? ApiProblem.fromHttpError(error) : error;
      if (problem instanceof ApiProblem && problem.code === 'unauthenticated' && !sessionCheck) {
        injector.get(AuthService).sessionEnded();
      }
      return throwError(() => problem);
    }),
  );
};
