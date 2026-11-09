/*
 * Public API Surface of ui
 */

// API client, generated from backend/openapi.json by `just generate-client`.
export * from './lib/api/generated';
export { ERROR_CODE } from './lib/api/generated/models/error-code-array';
export * from './lib/api/api-problem';
export * from './lib/api/provide-api';

// Translations, language, time zone and formatting.
export * from './lib/i18n/format';
export * from './lib/i18n/locale.service';
export * from './lib/i18n/pipes';
export * from './lib/i18n/provide-i18n';
export * from './lib/i18n/translated-title.strategy';

// Theme.
export * from './lib/theming/theme.service';
export * from './lib/theming/provide-material';

// Application shell.
export * from './lib/shell/nav-item';
export * from './lib/shell/shell';
export * from './lib/shell/language-menu';
export * from './lib/shell/theme-menu';
export * from './lib/shell/tenant-switcher';

// Shared components.
export * from './lib/components/status-chip';
export * from './lib/components/type-chip';
export * from './lib/components/empty-state';
export * from './lib/components/error-state';
export * from './lib/components/loading-state';
