import { Injectable, computed, inject, signal } from '@angular/core';
import { Router } from '@angular/router';

import { isApiProblem } from '../api/api-problem';
import { Api } from '../api/generated/api';
import {
  confirmPasswordReset,
  getSession,
  requestPasswordReset,
  signIn,
  signOut,
} from '../api/generated/functions';
import type { Session } from '../api/generated/models/session';

/** Route of the sign-in page in every application. */
export const SIGN_IN_PATH = '/sign-in';

/**
 * Whether `url` may be followed after sign-in: a path of this application
 * only, never another site (`//evil.example`, `https://…`).
 */
export function isSafeReturnUrl(url: unknown): url is string {
  return typeof url === 'string' && url.startsWith('/') && !url.startsWith('//') && !url.startsWith('/\\');
}

/**
 * The signed-in member and the authentication calls (specification §14).
 *
 * The session itself is an `HttpOnly` cookie the browser sends; this service
 * only knows whether there is one, and who it belongs to. State-changing
 * requests carry the CSRF token through `provideApi()`.
 */
@Injectable({ providedIn: 'root' })
export class AuthService {
  private readonly api = inject(Api);
  private readonly router = inject(Router);

  /** `undefined` until known, `null` when signed out. */
  private readonly state = signal<Session | null | undefined>(undefined);
  private loading: Promise<Session | null> | null = null;

  /** The current session, `null` when signed out, `undefined` until known. */
  readonly session = this.state.asReadonly();
  readonly signedIn = computed(() => !!this.state());

  /**
   * The current session, asked to the API the first time. Resolves to
   * `null` when signed out, or when the API cannot answer (it is asked
   * again next time).
   */
  async load(): Promise<Session | null> {
    const known = this.state();
    if (known !== undefined) {
      return known;
    }
    this.loading ??= this.api
      .invoke(getSession)
      .then(
        (session) => {
          this.state.set(session);
          return session;
        },
        (error: unknown) => {
          if (isApiProblem(error) && error.status === 401) {
            this.state.set(null);
          }
          return null;
        },
      )
      .finally(() => (this.loading = null));
    return this.loading;
  }

  /** Signs in; rejects with an `ApiProblem` (wrong credentials, 429…). */
  async signIn(email: string, password: string): Promise<Session> {
    const session = await this.api.invoke(signIn, { body: { email, password } });
    this.state.set(session);
    return session;
  }

  /** Signs out, then shows the sign-in page, even if the API failed. */
  async signOut(): Promise<void> {
    try {
      await this.api.invoke(signOut);
    } catch {
      // The session ends anyway on this side; the server forgets it later.
    } finally {
      this.state.set(null);
      await this.router.navigateByUrl(SIGN_IN_PATH);
    }
  }

  /** Asks for a reset link; the API accepts any email (no enumeration). */
  async requestPasswordReset(email: string): Promise<void> {
    await this.api.invoke(requestPasswordReset, { body: { email } });
  }

  /** Sets a new password with the token of a reset link. */
  async confirmPasswordReset(token: string, password: string): Promise<void> {
    await this.api.invoke(confirmPasswordReset, { body: { token, password } });
    this.state.set(null);
  }

  /**
   * The API answered 401 to a request: the session ended (expired, signed
   * out elsewhere, password reset). Shows the sign-in page, then comes back.
   */
  sessionEnded(): void {
    const wasSignedIn = this.state() !== null;
    this.state.set(null);
    const current = this.router.url;
    if (!wasSignedIn || current.startsWith(SIGN_IN_PATH)) {
      return;
    }
    void this.router.navigate([SIGN_IN_PATH], {
      queryParams: isSafeReturnUrl(current) && current !== '/' ? { returnUrl: current } : {},
    });
  }
}
