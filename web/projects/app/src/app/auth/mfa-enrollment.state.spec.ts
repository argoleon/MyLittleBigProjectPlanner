import type { MfaSetup } from 'ui';

import {
  EnrollmentState,
  INITIAL_ENROLLMENT,
  backupCodesFile,
  enrollmentReducer,
  groupedSecret,
  qrCodeDataUrl,
} from './mfa-enrollment.state';

const SETUP: MfaSetup = {
  otpauthUri: 'otpauth://totp/MLBPP:lucas%40acme.example?secret=JBSWY3DPEHPK3PXP',
  secret: 'JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP',
  qrCodeSvg: '<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>',
};

describe('enrollment steps', () => {
  it('go from a drawn secret to the backup codes', () => {
    let state: EnrollmentState = INITIAL_ENROLLMENT;
    state = enrollmentReducer(state, { type: 'setupLoaded', setup: SETUP });
    expect(state).toEqual({ step: 'scan', setup: SETUP, confirming: false, errorKey: null });
    state = enrollmentReducer(state, { type: 'confirming' });
    expect(state).toMatchObject({ step: 'scan', confirming: true });
    state = enrollmentReducer(state, { type: 'confirmFailed', errorKey: 'errors.auth.invalid_code' });
    expect(state).toEqual({ step: 'scan', setup: SETUP, confirming: false, errorKey: 'errors.auth.invalid_code' });
    state = enrollmentReducer(state, { type: 'confirming' });
    expect(state).toMatchObject({ errorKey: null });
    state = enrollmentReducer(state, { type: 'confirmed', backupCodes: ['abcde-fghjk'] });
    expect(state).toEqual({ step: 'codes', backupCodes: ['abcde-fghjk'] });
  });

  it('fail when no secret is drawn, and start over', () => {
    let state = enrollmentReducer(INITIAL_ENROLLMENT, { type: 'setupFailed', errorKey: 'errors.network' });
    expect(state).toEqual({ step: 'failed', errorKey: 'errors.network' });
    state = enrollmentReducer(state, { type: 'restart' });
    expect(state).toEqual(INITIAL_ENROLLMENT);
  });

  it('ignore events that do not belong to the current step', () => {
    const codes: EnrollmentState = { step: 'codes', backupCodes: ['abcde-fghjk'] };
    expect(enrollmentReducer(codes, { type: 'setupLoaded', setup: SETUP })).toBe(codes);
    expect(enrollmentReducer(codes, { type: 'confirmFailed', errorKey: 'x' })).toBe(codes);
    expect(enrollmentReducer(INITIAL_ENROLLMENT, { type: 'confirmed', backupCodes: [] })).toBe(INITIAL_ENROLLMENT);
    expect(enrollmentReducer(INITIAL_ENROLLMENT, { type: 'confirming' })).toBe(INITIAL_ENROLLMENT);
  });
});

describe('enrollment helpers', () => {
  it('show the QR code drawn by the API as an image', () => {
    const url = qrCodeDataUrl(SETUP);
    expect(url.startsWith('data:image/svg+xml;base64,')).toBe(true);
    expect(atob(url.split(',')[1])).toBe(SETUP.qrCodeSvg);
  });

  it('group the key by four characters', () => {
    expect(groupedSecret('JBSWY3DPEHPK3PXP')).toBe('JBSW Y3DP EHPK 3PXP');
    expect(groupedSecret('ABCDE')).toBe('ABCD E');
  });

  it('number the backup codes of the downloaded file', () => {
    expect(backupCodesFile(['abcde-fghjk', 'mnpqr-stuvw'], 'Backup codes')).toBe(
      'Backup codes\n\n1. abcde-fghjk\n2. mnpqr-stuvw\n',
    );
  });
});
