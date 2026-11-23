import { Routes } from '@angular/router';
import { authGuard, signedOutGuard, stageGuard } from 'ui';

import { adminHomeGuard } from './admin/admin-tabs';
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
      // Screens not built yet show "coming soon".
      ...NAVIGATION.filter((item) => !['/admin', '/boards', '/portfolios', '/search', '/tickets', '/workload'].includes(item.route)).map((item) => ({
        path: item.route.slice(1),
        pathMatch: 'full' as const,
        title: item.labelKey,
        data: { titleKey: item.labelKey, icon: item.icon },
        loadComponent: () => import('./coming-soon/coming-soon-page').then((m) => m.ComingSoonPage),
      })),
      {
        path: 'notifications',
        title: 'notifications.title',
        loadComponent: () => import('./notifications/notifications-page').then((m) => m.NotificationsPage),
      },
      {
        // `?q=`: the query to run.
        path: 'search',
        title: 'nav.search',
        loadComponent: () => import('./search/search-page').then((m) => m.SearchPage),
      },
      {
        path: 'boards',
        pathMatch: 'full',
        title: 'nav.boards',
        loadComponent: () => import('./boards/boards-page').then((m) => m.BoardsPage),
      },
      {
        // The report of a bulk action, where its notification leads.
        path: 'bulk-actions/:id',
        title: 'bulk.report.page',
        loadComponent: () => import('./bulk/bulk-action-page').then((m) => m.BulkActionPage),
      },
      {
        // An export of the member's, where its notification leads.
        path: 'exports/:id',
        title: 'exports.title',
        loadComponent: () => import('./exports/export-page').then((m) => m.ExportPage),
      },
      {
        // A Kanban board, by its id.
        path: 'boards/:id',
        title: 'nav.boards',
        loadComponent: () => import('./boards/board-page').then((m) => m.BoardPage),
      },
      {
        // Workload: its tabs (the heatmap comes with L08).
        path: 'workload',
        title: 'nav.workload',
        loadComponent: () => import('./workload/workload-page').then((m) => m.WorkloadPage),
        children: [
          { path: '', pathMatch: 'full', redirectTo: 'resources' },
          {
            path: 'resources',
            title: 'resources.title',
            loadComponent: () => import('./resources/resources-page').then((m) => m.ResourcesPage),
          },
          {
            path: 'teams',
            title: 'teams.title',
            loadComponent: () => import('./teams/teams-page').then((m) => m.TeamsPage),
          },
        ],
      },
      {
        // A resource, by its id.
        path: 'workload/resources/:id',
        title: 'resources.title',
        loadComponent: () => import('./resources/resource-page').then((m) => m.ResourcePage),
      },
      {
        // A team and its members, by its id.
        path: 'workload/teams/:id',
        title: 'teams.title',
        loadComponent: () => import('./teams/team-page').then((m) => m.TeamPage),
      },
      {
        path: 'tickets',
        title: 'nav.myTickets',
        loadComponent: () => import('./issues/my-tickets-page').then((m) => m.MyTicketsPage),
      },
      {
        // A ticket, by its key (or a former key, or its id).
        path: 'issues/:key',
        title: 'issue.page.title',
        loadComponent: () => import('./issues/issue-page').then((m) => m.IssuePage),
      },
      {
        path: 'portfolios',
        title: 'nav.portfolios',
        loadComponent: () => import('./hierarchy/portfolios-page').then((m) => m.PortfoliosPage),
      },
      {
        // The view of a node; its settings are the routes below.
        path: 'nodes/:id',
        pathMatch: 'full',
        title: 'nav.portfolios',
        loadComponent: () => import('./hierarchy/node-page').then((m) => m.NodePage),
      },
      {
        path: 'nodes/:id',
        loadComponent: () => import('./hierarchy/node-settings-page').then((m) => m.NodeSettingsPage),
        children: [
          {
            path: 'general',
            title: 'hierarchy.settings.general',
            loadComponent: () => import('./hierarchy/node-general-tab').then((m) => m.NodeGeneralTab),
          },
          {
            path: 'links',
            title: 'hierarchy.settings.links',
            loadComponent: () => import('./hierarchy/node-links-tab').then((m) => m.NodeLinksTab),
          },
          {
            path: 'configuration',
            title: 'hierarchy.settings.configuration',
            loadComponent: () => import('./hierarchy/node-configuration-tab').then((m) => m.NodeConfigurationTab),
          },
          {
            path: 'archiving',
            title: 'hierarchy.settings.archiving',
            loadComponent: () => import('./hierarchy/node-archiving-tab').then((m) => m.NodeArchivingTab),
          },
        ],
      },
      {
        path: 'account',
        title: 'account.title',
        loadComponent: () => import('./account/account-page').then((m) => m.AccountPage),
      },
      {
        // A project configuration (mockup 11): its own screen, out of the admin tabs.
        path: 'admin/configurations/:id',
        loadComponent: () => import('./configuration/config-page').then((m) => m.ConfigPage),
        children: [
          { path: '', pathMatch: 'full', redirectTo: 'types' },
          {
            path: 'types',
            title: 'config.sections.types.tab',
            loadComponent: () => import('./configuration/types-tab').then((m) => m.TypesTab),
          },
          {
            path: 'fields',
            title: 'config.sections.fields.tab',
            loadComponent: () => import('./configuration/fields-tab').then((m) => m.FieldsTab),
          },
          {
            path: 'workflows',
            title: 'config.sections.workflows.tab',
            loadComponent: () => import('./configuration/workflows-tab').then((m) => m.WorkflowsTab),
          },
          {
            path: 'screens',
            title: 'config.sections.screens.tab',
            loadComponent: () => import('./configuration/screens-tab').then((m) => m.ScreensTab),
          },
          {
            path: 'notifications',
            title: 'config.sections.notifications.tab',
            loadComponent: () => import('./configuration/notifications-tab').then((m) => m.NotificationsTab),
          },
          {
            path: 'projects',
            title: 'config.projects.tab',
            loadComponent: () => import('./configuration/config-projects-tab').then((m) => m.ConfigProjectsTab),
          },
        ],
      },
      {
        path: 'admin',
        loadComponent: () => import('./admin/admin-page').then((m) => m.AdminPage),
        children: [
          // The first tab of the member's role.
          { path: '', pathMatch: 'full', canActivate: [adminHomeGuard], children: [] },
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
          {
            path: 'configurations',
            title: 'admin.tabs.configurations',
            loadComponent: () => import('./configuration/configurations-tab').then((m) => m.ConfigurationsTab),
          },
        ],
      },
    ],
  },
  { path: '**', redirectTo: '' },
];
