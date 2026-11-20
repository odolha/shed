import en from './en.json';
import { setDictionary, t } from './i18n';

describe('t', () => {
  afterEach(() => setDictionary(en as Record<string, string>));

  it('looks keys up and fills params', () => {
    setDictionary({ hello: 'Hello, {name}!', plain: 'Plain' });
    expect(t('plain')).toBe('Plain');
    expect(t('hello', { name: 'you' })).toBe('Hello, you!');
  });

  it('shows a missing key as itself, and leaves unknown params', () => {
    setDictionary({ x: 'a {b} c' });
    expect(t('nope.missing')).toBe('nope.missing');
    expect(t('x', {})).toBe('a {b} c');
  });

  it('picks the plural form from n', () => {
    setDictionary({ 'files.one': '{n} file', 'files.other': '{n} files', count: '{n} of them' });
    expect(t('files', { n: 1 })).toBe('1 file');
    expect(t('files', { n: 3 })).toBe('3 files');
    expect(t('count', { n: 2 })).toBe('2 of them');
  });

  it('has every string the app asks for by a fixed key', () => {
    const dict = en as Record<string, string>;
    for (const key of ['common.cancel', 'error.forbidden', 'ai.allow', 'ai.deny', 'ai.allowSession', 'bench.close']) {
      expect(dict[key], key).toBeTruthy();
    }
    for (const code of ['unauthorized', 'forbidden', 'notFound', 'offline', 'invalid', 'unsupported', 'rateLimited', 'wrongCode', 'timeout', 'failed']) {
      expect(dict[`error.${code}`], code).toBeTruthy();
    }
  });
});
