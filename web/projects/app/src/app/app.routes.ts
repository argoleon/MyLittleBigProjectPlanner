import { Routes } from '@angular/router';

import { NAVIGATION } from './navigation';

/**
 * Every screen is a placeholder until its work package; `title` is a
 * translation key (see `TranslatedTitleStrategy`).
 */
export const routes: Routes = [
  ...NAVIGATION.map((item) => ({
    path: item.route.slice(1),
    pathMatch: 'full' as const,
    title: item.labelKey,
    data: { titleKey: item.labelKey, icon: item.icon },
    loadComponent: () => import('./coming-soon/coming-soon-page').then((m) => m.ComingSoonPage),
  })),
  { path: '**', redirectTo: '' },
];
