/** An entry of the navigation drawer. */
export interface NavItem {
  /** Router link, e.g. `/workload`. */
  readonly route: string;
  /** Translation key of the label, e.g. `nav.workload`. */
  readonly labelKey: string;
  /** Material Symbols name, e.g. `groups`. */
  readonly icon: string;
  /** Active only on this exact URL (for the home entry). */
  readonly exact?: boolean;
}
