import { normalizePairCode } from './approve-dialog';

describe('normalizePairCode', () => {
  it('takes the code in any case, with or without its dash', () => {
    expect(normalizePairCode('k7qf-2m9d')).toBe('K7QF-2M9D');
    expect(normalizePairCode(' k7qf2m9d ')).toBe('K7QF-2M9D');
    expect(normalizePairCode('K7QF 2M9D')).toBe('K7QF-2M9D');
  });

  it('leaves other shapes as typed, upper case', () => {
    expect(normalizePairCode('abc123')).toBe('ABC123');
  });
});
