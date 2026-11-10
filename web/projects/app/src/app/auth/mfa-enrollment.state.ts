import type { MfaSetup } from 'ui';

/**
 * Steps of enabling two-step verification:
 *
 * 1. `starting`: the API draws a secret;
 * 2. `scan`: the member adds it to the app (QR code or key) and types a code;
 * 3. `codes`: enabled, the backup codes are shown once.
 *
 * `failed` when no secret could be drawn (retry starts over).
 */
export type EnrollmentState =
  | { readonly step: 'starting' }
  | { readonly step: 'scan'; readonly setup: MfaSetup; readonly confirming: boolean; readonly errorKey: string | null }
  | { readonly step: 'codes'; readonly backupCodes: readonly string[] }
  | { readonly step: 'failed'; readonly errorKey: string };

export type EnrollmentEvent =
  | { readonly type: 'restart' }
  | { readonly type: 'setupLoaded'; readonly setup: MfaSetup }
  | { readonly type: 'setupFailed'; readonly errorKey: string }
  | { readonly type: 'confirming' }
  | { readonly type: 'confirmFailed'; readonly errorKey: string }
  | { readonly type: 'confirmed'; readonly backupCodes: readonly string[] };

export const INITIAL_ENROLLMENT: EnrollmentState = { step: 'starting' };

/** The state after `event`; events that do not apply to a step are ignored. */
export function enrollmentReducer(state: EnrollmentState, event: EnrollmentEvent): EnrollmentState {
  switch (event.type) {
    case 'restart':
      return INITIAL_ENROLLMENT;
    case 'setupLoaded':
      return state.step === 'starting'
        ? { step: 'scan', setup: event.setup, confirming: false, errorKey: null }
        : state;
    case 'setupFailed':
      return state.step === 'starting' ? { step: 'failed', errorKey: event.errorKey } : state;
    case 'confirming':
      return state.step === 'scan' ? { ...state, confirming: true, errorKey: null } : state;
    case 'confirmFailed':
      return state.step === 'scan' ? { ...state, confirming: false, errorKey: event.errorKey } : state;
    case 'confirmed':
      return state.step === 'scan' ? { step: 'codes', backupCodes: event.backupCodes } : state;
  }
}

/** The QR code of `setup` as an image URL (the SVG drawn by the API). */
export function qrCodeDataUrl(setup: MfaSetup): string {
  const bytes = new TextEncoder().encode(setup.qrCodeSvg);
  let binary = '';
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return `data:image/svg+xml;base64,${btoa(binary)}`;
}

/** The secret in groups of 4 characters, easier to type. */
export function groupedSecret(secret: string): string {
  return secret.replace(/(.{4})(?=.)/g, '$1 ');
}

/** Text of the file the backup codes can be downloaded as. */
export function backupCodesFile(codes: readonly string[], title: string): string {
  return `${title}\n\n${codes.map((code, i) => `${i + 1}. ${code}`).join('\n')}\n`;
}
