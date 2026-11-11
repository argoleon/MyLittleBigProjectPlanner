import { Routes } from '@angular/router';
import { authGuard, signedOutGuard, stageGuard } from 'ui';

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
    path: 'sign-in/verify',
    title: 'auth.verify.title',
    canActivate: [stageGuard('mfa_challenge')],
    loadComponent: () => import('./auth/verify-page').then((m) => m.VerifyPage),
  },
  {
    path: 'sign-in/enroll',
    title: 'auth.enroll.title',
    canActivate: [stageGuard('mfa_enrollment')],
    loadComponent: () => import('./auth/enroll-page').then((m) => m.EnrollPage),
  },
  {
    path: 'forgot-password',
    title: 'auth.forgotPassword.title',
    loadComponent: () => import('./auth/forgot-password-page').then((m) => m.ForgotPasswordPage),
  },
  {
    path: 'accept-invitation',
    title: 'auth.acceptInvitation.title',
    loadComponent: () => import('./auth/accept-invitation-page').then((m) => m.AcceptInvitationPage),
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
    children: [
      ...NAVIGATION.filter((item) => item.route !== '/admin').map((item) => ({
        path: item.route.slice(1),
        pathMatch: 'full' as const,
        title: item.labelKey,
        data: { titleKey: item.labelKey, icon: item.icon },
        loadComponent: () => import('./coming-soon/coming-soon-page').then((m) => m.ComingSoonPage),
      })),
      {
        path: 'admin',
        loadComponent: () => import('./admin/admin-page').then((m) => m.AdminPage),
        children: [
          { path: '', pathMatch: 'full', redirectTo: 'members' },
          {
            path: 'members',
            title: 'admin.tabs.members',
            loadComponent: () => import('./admin/members-tab').then((m) => m.MembersTab),
          },
          {
            path: 'groups',
            title: 'admin.tabs.groups',
            loadComponent: () => import('./admin/groups-tab').then((m) => m.GroupsTab),
          },
          {
            path: 'roles',
            title: 'admin.tabs.roles',
            loadComponent: () => import('./admin/roles-tab').then((m) => m.RolesTab),
          },
          {
            path: 'authentication',
            title: 'admin.tabs.authentication',
            loadComponent: () => import('./admin/authentication-settings').then((m) => m.AuthenticationSettings),
          },
        ],
      },
    ],
  },
  { path: '**', redirectTo: '' },
];
