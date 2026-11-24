import { commonJs, groupSecret, qrDataUrl } from './setup';

describe('setup', () => {
  it('draws the otpauth URL as an SVG data URL', async () => {
    const url = await qrDataUrl('otpauth://totp/Shed:you?secret=JBSWY3DPEHPK3PXP&issuer=Shed&digits=6&period=30&algorithm=SHA1');
    expect(url).not.toBeNull();
    expect(url!.startsWith('data:image/svg+xml;charset=utf-8,')).toBe(true);
    const svg = decodeURIComponent(url!.slice(url!.indexOf(',') + 1));
    expect(svg).toContain('<svg');
    expect(svg).toContain('<path');
  });

  it('takes a CommonJS module from its default export when that is all there is', () => {
    const exports = { toString: () => 'svg' };
    // What a production chunk gives: a namespace with only `default`.
    const namespace = Object.assign(Object.create(null), { default: exports });
    expect(commonJs(namespace)).toBe(exports);
    expect(commonJs(exports)).toBe(exports);
  });

  it('groups the key in fours', () => {
    expect(groupSecret('JBSWY3DPEHPK3PXP')).toBe('JBSW Y3DP EHPK 3PXP');
    expect(groupSecret('ABCDEF')).toBe('ABCD EF');
  });
});
