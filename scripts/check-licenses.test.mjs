import { describe, expect, it } from 'vitest';
import { isAccepted, unacceptedPackages } from './check-licenses.mjs';

const accepted = ['MIT', 'Apache-2.0'];

function lockWith(packages) {
  return { packages: { '': { name: 'marlin-file-browser' }, ...packages } };
}

describe('isAccepted', () => {
  it.each([
    'MIT',
    'mit',
    'GPL-3.0-only OR MIT',
    '(MIT OR GPL-3.0-or-later)',
    '(MIT AND Apache-2.0)',
    'MIT AND GPL-3.0-only OR Apache-2.0',
    '(GPL-3.0-only OR MIT) AND Apache-2.0',
  ])('accepts %s', (expression) => {
    expect(isAccepted(expression, accepted)).toBe(true);
  });

  it.each([
    'GPL-3.0-only',
    'MIT AND GPL-3.0-only',
    '(MIT OR Apache-2.0) AND GPL-3.0-only',
    'GPL-3.0-only WITH Classpath-exception-2.0 OR LGPL-2.1-only',
    'MIT WITH LLVM-exception',
    'SEE LICENSE IN LICENSE.txt',
    '(MIT',
    '(MIT GPL-3.0-only',
    'MIT GPL-3.0-only',
    'MIT OR AND',
    'MIT OR',
    'MIT WITH',
    '',
  ])('rejects %s', (expression) => {
    expect(isAccepted(expression, accepted)).toBe(false);
  });
});

describe('unacceptedPackages', () => {
  it('accepts a package whose license can be met from the list', () => {
    const lock = lockWith({
      'node_modules/react': { version: '19.0.0', license: 'MIT' },
      'node_modules/@tauri-apps/api': { version: '2.0.0', license: '(GPL-3.0-only OR MIT)' },
    });
    expect(unacceptedPackages(lock, accepted)).toEqual([]);
  });

  it('reports a shipped package under a license that is not on the list', () => {
    const lock = lockWith({
      'node_modules/a/node_modules/copyleft': { version: '1.2.3', license: 'GPL-3.0-only' },
      'node_modules/both': { version: '1.0.0', license: 'MIT AND GPL-3.0-only' },
    });
    expect(unacceptedPackages(lock, accepted)).toEqual([
      'copyleft@1.2.3 is under GPL-3.0-only',
      'both@1.0.0 is under MIT AND GPL-3.0-only',
    ]);
  });

  it('accepts a package that states no license and is listed in UNSTATED', () => {
    const lock = lockWith({ 'node_modules/@crabnebula/tauri-plugin-drag': { version: '2.1.0' } });
    expect(unacceptedPackages(lock, accepted)).toEqual([]);
  });

  it('reports a shipped package that states no license', () => {
    const lock = lockWith({ 'node_modules/@scope/mystery': { version: '0.1.0' } });
    expect(unacceptedPackages(lock, accepted)).toEqual(['@scope/mystery@0.1.0 states no license']);
  });

  it('reports a build tool that adds its own code to the bundle', () => {
    const lock = lockWith({
      'node_modules/rolldown': { version: '1.2.19', license: 'GPL-3.0-only', dev: true },
    });
    expect(unacceptedPackages(lock, accepted)).toEqual(['rolldown@1.2.19 is under GPL-3.0-only']);
  });

  it('skips a package that only the build or the tests use', () => {
    const lock = lockWith({
      'node_modules/eslint-thing': { version: '1.0.0', license: 'GPL-3.0-only', dev: true },
    });
    expect(unacceptedPackages(lock, accepted)).toEqual([]);
  });

  it('reports a package that is both a dev dependency and an optional one', () => {
    const lock = lockWith({
      'node_modules/peer-thing': { version: '1.0.0', license: 'GPL-3.0-only', devOptional: true },
    });
    expect(unacceptedPackages(lock, accepted)).toEqual(['peer-thing@1.0.0 is under GPL-3.0-only']);
  });
});
