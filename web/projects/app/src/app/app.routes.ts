import { Routes } from '@angular/router';
import { authGuard, signedOutGuard } from 'ui';

import { MainLayout } from './layout/main-layout';
import { NAVIGATION } from './navigation';

/**
 * The pages before signing in, then every screen of signed-in members
 * inside the main layout. Screens are placeholders until their work
 * package; `title` is a translation key (see `TranslatedTitleStrategy`).
 */
export const routes: Routes = [
  {
    path: 'sign-in',
    title: 'auth.signIn.title',
    canActivate: [signedOutGuard],
    loadComponent: () => import('./auth/sign-in-page').then((m) => m.SignInPage),
  },
  {
    path: 'forgot-password',
    title: 'auth.forgotPassword.title',
    loadComponent: () => import('./auth/forgot-password-page').then((m) => m.ForgotPasswordPage),
  },
  {
    path: 'reset-password',
    title: 'auth.resetPassword.title',
    loadComponent: () => import('./auth/reset-password-page').then((m) => m.ResetPasswordPage),
  },
  {
    path: '',
    component: MainLayout,
    canActivate: [authGuard],
    children: NAVIGATION.map((item) => ({
      path: item.route.slice(1),
      pathMatch: 'full' as const,
      title: item.labelKey,
      data: { titleKey: item.labelKey, icon: item.icon },
      loadComponent: () => import('./coming-soon/coming-soon-page').then((m) => m.ComingSoonPage),
    })),
  },
  { path: '**', redirectTo: '' },
];
