import { DOCUMENT, Injectable, InjectionToken, computed, inject, signal } from '@angular/core';
import { Router } from '@angular/router';

import { isApiProblem } from '../api/api-problem';
import { Api } from '../api/generated/api';
import {
  acceptInvitation,
  confirmMfa,
  getInvitation,
  confirmPasswordReset,
  getSession,
  requestPasswordReset,
  signIn,
  signOut,
  switchTenant,
  verifySignIn,
} from '../api/generated/functions';
import type { InvitationAccepted } from '../api/generated/models/invitation-accepted';
import type { InvitationPreview } from '../api/generated/models/invitation-preview';
import type { MfaEnabled } from '../api/generated/models/mfa-enabled';
import type { Session } from '../api/generated/models/session';
import type { SessionStage } from '../api/generated/models/session-stage';

/** Route of the sign-in page in every application. */
export const SIGN_IN_PATH = '/sign-in';

/** Route of the two-step verification code, after the password. */
export const VERIFY_PATH = '/sign-in/verify';

/** Route of the enrollment the tenant requires, after the password. */
export const ENROLL_PATH = '/sign-in/enroll';

/** Where a session at `stage` goes next: `null` once signed in. */
export function pathForStage(stage: SessionStage): string | null {
  switch (stage) {
    case 'mfa_challenge':
      return VERIFY_PATH;
    case 'mfa_enrollment':
      return ENROLL_PATH;
    default:
      return null;
  }
}

/**
 * Whether `url` may be followed after sign-in: a path of this application
 * only, never another site (`//evil.example`, `https://…`).
 */
export function isSafeReturnUrl(url: unknown): url is string {
  return typeof url === 'string' && url.startsWith('/') && !url.startsWith('//') && !url.startsWith('/\\');
}

/**
 * Leaves the application for `url` (another tenant's address): a full page
 * load, so that nothing of the current tenant stays in memory. Replaced in
 * tests.
 */
export const LEAVE_APPLICATION = new InjectionToken<(url: string) => void>('LEAVE_APPLICATION', {
  providedIn: 'root',
  factory: () => {
    const document = inject(DOCUMENT);
    return (url: string) => document.location.assign(url);
  },
});

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
  private readonly leave = inject(LEAVE_APPLICATION);

  /** `undefined` until known, `null` when signed out. */
  private readonly state = signal<Session | null | undefined>(undefined);
  private loading: Promise<Session | null> | null = null;

  /**
   * The current session, `null` when signed out, `undefined` until known.
   * A session may still wait for two-step verification: see `stage`.
   */
  readonly session = this.state.asReadonly();
  /** Fully signed in: the application is reachable. */
  readonly signedIn = computed(() => this.state()?.stage === 'complete');

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

  /**
   * Checks the password; rejects with an `ApiProblem` (wrong credentials,
   * 429…). The session's `stage` tells whether a second factor follows.
   */
  async signIn(email: string, password: string): Promise<Session> {
    const session = await this.api.invoke(signIn, { body: { email, password } });
    this.state.set(session);
    return session;
  }

  /** Completes sign-in with a TOTP or backup code. */
  async verifySignIn(code: string): Promise<Session> {
    const session = await this.api.invoke(verifySignIn, { body: { code } });
    this.state.set(session);
    return session;
  }

  /**
   * Enables two-step verification with a first code. A session waiting for
   * this enrollment becomes complete.
   */
  async confirmMfa(code: string): Promise<MfaEnabled> {
    const enabled = await this.api.invoke(confirmMfa, { body: { code } });
    this.state.set(enabled.session);
    return enabled;
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

  /**
   * Switches to another tenant of the account: the API issues a one-time
   * link to it, then the client state is cleared and the page leaves for
   * that tenant, which opens its own session under its own policy. Rejects
   * with an `ApiProblem`, keeping the current session, if the API refuses.
   */
  async switchTenant(tenantId: string): Promise<void> {
    const { url } = await this.api.invoke(switchTenant, { body: { tenantId } });
    // Nothing of this tenant must survive the switch.
    this.loading = null;
    this.state.set(null);
    this.leave(url);
  }

  /** The invitation of an emailed link; rejects with `invitation.invalid`. */
  async invitation(token: string): Promise<InvitationPreview> {
    return this.api.invoke(getInvitation, { body: { token } });
  }

  /**
   * Accepts an invitation, with a name and a password for a new account.
   * An internal member comes back signed in (maybe at a two-step
   * verification stage): the session is taken at once.
   */
  async acceptInvitation(token: string, account?: { displayName: string; password: string }): Promise<InvitationAccepted> {
    const accepted = await this.api.invoke(acceptInvitation, { body: { token, ...account } });
    if (accepted.session) {
      this.state.set(accepted.session);
    }
    return accepted;
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
