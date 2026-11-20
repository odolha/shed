import type { PendingPairing } from '../../api/gen/api';
import { rejectBody } from './devices';

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
