import {
  HttpErrorResponse,
  HttpInterceptorFn,
  provideHttpClient,
  withFetch,
  withInterceptors,
} from '@angular/common/http';
import { EnvironmentProviders, inject, makeEnvironmentProviders } from '@angular/core';
import { catchError, throwError } from 'rxjs';

import { ApiProblem } from './api-problem';
import { ApiConfiguration, provideApiConfiguration } from './generated/api-configuration';

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
 * URL, session cookies, and errors turned into {@link ApiProblem}.
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
    provideHttpClient(withFetch(), withInterceptors([apiInterceptor])),
  ]);
}

/**
 * For requests to the API only: sends the session cookie, and turns error
 * responses into {@link ApiProblem}. Other requests (translation files,
 * assets) are left untouched.
 */
export const apiInterceptor: HttpInterceptorFn = (request, next) => {
  const prefix = `${inject(ApiConfiguration).rootUrl}/api/`;
  if (!request.url.startsWith(prefix)) {
    return next(request);
  }
  return next(request.clone({ withCredentials: true })).pipe(
    catchError((error: unknown) =>
      throwError(() =>
        error instanceof HttpErrorResponse ? ApiProblem.fromHttpError(error) : error,
      ),
    ),
  );
};
