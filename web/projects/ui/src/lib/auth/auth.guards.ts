import { inject } from '@angular/core';
import { CanActivateFn, Router } from '@angular/router';

import type { SessionStage } from '../api/generated/models/session-stage';
import { AuthService, SIGN_IN_PATH, isSafeReturnUrl, pathForStage } from './auth.service';

/**
 * Lets fully signed-in members through; sends anyone else to the sign-in
 * page, or to the two-step verification step their session waits for, which
 * come back here afterwards.
 */
export const authGuard: CanActivateFn = async (_route, state) => {
  const auth = inject(AuthService);
  const router = inject(Router);
  const session = await auth.load();
  const returnUrl = state.url !== '/' && isSafeReturnUrl(state.url) ? { returnUrl: state.url } : undefined;
  if (!session) {
    return router.createUrlTree([SIGN_IN_PATH], { queryParams: returnUrl });
  }
  const next = pathForStage(session.stage);
  return next ? router.createUrlTree([next], { queryParams: returnUrl }) : true;
};

/** The sign-in page is for signed-out visitors: members go home. */
export const signedOutGuard: CanActivateFn = async () => {
  const auth = inject(AuthService);
  const router = inject(Router);
  return (await auth.load())?.stage === 'complete' ? router.createUrlTree(['/']) : true;
};

/** A two-step verification page, for a session at `stage` only. */
export function stageGuard(stage: SessionStage): CanActivateFn {
  return async () => {
    const auth = inject(AuthService);
    const router = inject(Router);
    return (await auth.load())?.stage === stage ? true : router.createUrlTree([SIGN_IN_PATH]);
  };
}
