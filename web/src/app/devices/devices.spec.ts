import type { PendingPairing } from '../../api/gen/api';
import { rejectBody, signInEnds } from './devices';

describe('rejectBody', () => {
  const request: PendingPairing = { code: '', name: 'buildbox', version: '0.1.0', address: '192.0.2.15', expires: 0 };

  it('rejects by the id the list gives, never a code', () => {
    expect(rejectBody({ ...request, id: 'p-1' })).toEqual({ id: 'p-1' });
  });

  it('cannot reject a request without an id from the list', () => {
    expect(rejectBody(request)).toBeNull();
    expect(rejectBody({ ...request, id: '' })).toBeNull();
  });
});

describe('signInEnds', () => {
  it('says when a phone or desktop sign-in ends', () => {
    expect(signInEnds(1000 + 7 * 86400, 1000)).toBe('Sign-in ends in 7d');
    expect(signInEnds(1000 + 3600, 1000)).toBe('Sign-in ends in 1h');
    expect(signInEnds(999, 1000)).toBe('Sign-in ended');
  });

  it('says nothing for devices whose sign-in does not end', () => {
    expect(signInEnds(null, 1000)).toBe('');
    expect(signInEnds(undefined, 1000)).toBe('');
  });
});
