// @ts-check
import eslint from '@eslint/js';
import angular from 'angular-eslint';
import { defineConfig } from 'eslint/config';
import tseslint from 'typescript-eslint';

/** Selector prefix of each project. */
const PREFIXES = { app: 'app', portal: 'portal', ui: 'ui' };

export default defineConfig([
  { ignores: ['dist/', '.angular/', 'projects/ui/src/lib/api/generated/'] },
  {
    files: ['**/*.ts'],
    extends: [
      eslint.configs.recommended,
      tseslint.configs.recommended,
      tseslint.configs.stylistic,
      angular.configs.tsRecommended,
    ],
    processor: angular.processInlineTemplates,
    rules: {
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      // Styles live in .scss files, where stylelint forbids hard-coded values.
      '@angular-eslint/component-max-inline-declarations': ['error', { template: 60, styles: 0, animations: 0 }],
    },
  },
  ...Object.entries(PREFIXES).map(([project, prefix]) => ({
    files: [`projects/${project}/src/**/*.ts`],
    ignores: ['**/*.spec.ts'],
    rules: {
      '@angular-eslint/component-selector': ['error', { type: 'element', prefix, style: 'kebab-case' }],
      '@angular-eslint/directive-selector': ['error', { type: 'attribute', prefix, style: 'camelCase' }],
    },
  })),
  {
    // Applications reach the API through the generated client (`Api`), never
    // through HttpClient (CLAUDE.md, "Frontend rules").
    files: ['projects/app/src/**/*.ts', 'projects/portal/src/**/*.ts'],
    ignores: ['**/*.spec.ts'],
    rules: {
      'no-restricted-imports': [
        'error',
        {
          paths: [
            {
              name: '@angular/common/http',
              importNames: ['HttpClient'],
              message: 'Call the API through the generated client: inject(Api) from "ui".',
            },
          ],
        },
      ],
    },
  },
  {
    files: ['**/*.html'],
    extends: [angular.configs.templateRecommended, angular.configs.templateAccessibility],
  },
]);
