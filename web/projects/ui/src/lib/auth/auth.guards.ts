import { inject } from '@angular/core';
import { CanActivateFn, Router } from '@angular/router';

import { AuthService, SIGN_IN_PATH, isSafeReturnUrl } from './auth.service';

/**
 * Lets signed-in members through; sends anyone else to the sign-in page,
 * which comes back here afterwards.
 */
export const authGuard: CanActivateFn = async (_route, state) => {
  const auth = inject(AuthService);
  const router = inject(Router);
  if (await auth.load()) {
    return true;
  }
  const returnUrl = state.url !== '/' && isSafeReturnUrl(state.url) ? { returnUrl: state.url } : undefined;
  return router.createUrlTree([SIGN_IN_PATH], { queryParams: returnUrl });
};

/** The sign-in page is for signed-out visitors: members go home. */
export const signedOutGuard: CanActivateFn = async () => {
  const auth = inject(AuthService);
  const router = inject(Router);
  return (await auth.load()) ? router.createUrlTree(['/']) : true;
};
