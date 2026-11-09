import { Routes } from '@angular/router';

/** `title` is a translation key (see `TranslatedTitleStrategy`). */
export const routes: Routes = [
  {
    path: '',
    pathMatch: 'full',
    title: 'page.portalHome.title',
    loadComponent: () => import('./home/home-page').then((m) => m.HomePage),
  },
  { path: '**', redirectTo: '' },
];
