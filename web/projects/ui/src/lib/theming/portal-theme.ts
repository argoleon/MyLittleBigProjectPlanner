/**
 * Runtime Material 3 theming for portals.
 *
 * Each portal has its own brand color. The full M3 color scheme (light and
 * dark) is derived from it with @material/material-color-utilities
 * (Apache-2.0) and applied as --mat-sys-* overrides on the portal's root
 * element, so every Angular Material component and every --app-* token
 * follows the portal brand without any extra stylesheet.
 *
 * Usage, in the portal shell component:
 *
 *   private readonly host = inject(ElementRef<HTMLElement>);
 *   constructor() {
 *     effect(() => applyPortalTheme(this.host.nativeElement, this.portal().brandColor));
 *   }
 *
 * The values come from the M3 dynamic scheme and can differ by a shade from
 * the static values emitted by mat.theme() for the same color (mostly in dark
 * mode). Portals only ever use this helper, so they stay consistent.
 */
import {
  argbFromHex,
  DynamicScheme,
  hexFromArgb,
  Hct,
  SchemeTonalSpot,
} from '@material/material-color-utilities';

/** M3 color roles exposed by Angular Material as --mat-sys-<role>. */
const ROLES = [
  'primary', 'onPrimary', 'primaryContainer', 'onPrimaryContainer', 'inversePrimary',
  'primaryFixed', 'primaryFixedDim', 'onPrimaryFixed', 'onPrimaryFixedVariant',
  'secondary', 'onSecondary', 'secondaryContainer', 'onSecondaryContainer',
  'secondaryFixed', 'secondaryFixedDim', 'onSecondaryFixed', 'onSecondaryFixedVariant',
  'tertiary', 'onTertiary', 'tertiaryContainer', 'onTertiaryContainer',
  'tertiaryFixed', 'tertiaryFixedDim', 'onTertiaryFixed', 'onTertiaryFixedVariant',
  'background', 'onBackground', 'surface', 'surfaceDim', 'surfaceBright',
  'surfaceContainerLowest', 'surfaceContainerLow', 'surfaceContainer',
  'surfaceContainerHigh', 'surfaceContainerHighest', 'onSurface',
  'surfaceVariant', 'onSurfaceVariant', 'inverseSurface', 'inverseOnSurface',
  'outline', 'outlineVariant', 'surfaceTint', 'shadow', 'scrim',
  'error', 'onError', 'errorContainer', 'onErrorContainer',
] as const;

type Role = (typeof ROLES)[number];

const toCssName = (role: string): string =>
  '--mat-sys-' + role.replace(/[A-Z]/g, (c) => '-' + c.toLowerCase());

/** Returns the --mat-sys-* declarations for a brand color, as light-dark() pairs. */
export function portalThemeVariables(brandHex: string): Record<string, string> {
  const source = Hct.fromInt(argbFromHex(brandHex));
  const light: DynamicScheme = new SchemeTonalSpot(source, false, 0);
  const dark: DynamicScheme = new SchemeTonalSpot(source, true, 0);
  const vars: Record<string, string> = {};
  for (const role of ROLES) {
    const l = (light as unknown as Record<Role, number>)[role];
    const d = (dark as unknown as Record<Role, number>)[role];
    vars[toCssName(role)] = `light-dark(${hexFromArgb(l)}, ${hexFromArgb(d)})`;
  }
  return vars;
}

/** Applies a portal brand color to an element and its descendants. */
export function applyPortalTheme(element: HTMLElement, brandHex: string): void {
  for (const [name, value] of Object.entries(portalThemeVariables(brandHex))) {
    element.style.setProperty(name, value);
  }
}
