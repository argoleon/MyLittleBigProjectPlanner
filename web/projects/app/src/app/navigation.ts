import { NavItem } from 'ui';

/** Entries of the navigation drawer, in the order of the mockups. */
export const NAVIGATION: readonly NavItem[] = [
  { route: '/', labelKey: 'nav.home', icon: 'home', exact: true },
  { route: '/tickets', labelKey: 'nav.myTickets', icon: 'assignment' },
  { route: '/search', labelKey: 'nav.search', icon: 'search' },
  { route: '/boards', labelKey: 'nav.boards', icon: 'view_kanban' },
  { route: '/gantt', labelKey: 'nav.gantt', icon: 'view_timeline' },
  { route: '/workload', labelKey: 'nav.workload', icon: 'groups' },
  { route: '/portfolios', labelKey: 'nav.portfolios', icon: 'account_tree' },
  { route: '/portals', labelKey: 'nav.portals', icon: 'support_agent' },
  { route: '/dashboards', labelKey: 'nav.dashboards', icon: 'dashboard' },
  { route: '/admin', labelKey: 'nav.administration', icon: 'settings' },
];
