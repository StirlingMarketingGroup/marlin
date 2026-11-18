import { afterEach, describe, expect, it } from 'vitest';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { npmSection, rustSection, SECTIONS, crateFilesSection } from './generate-license-texts.mjs';

const rule = '-'.repeat(80);

function tempDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'license-texts-'));
}

describe('rustSection', () => {
  it('gives each license a heading and the crates that use it', () => {
    const text = rustSection({
      licenses: [
        {
          name: 'MIT License',
          id: 'MIT',
          text: 'Permission is granted.',
          used_by: [
            { crate: { name: 'libc', version: '0.2.0' } },
            { crate: { name: 'serde', version: '1.0.0' } },
          ],
        },
        {
          name: 'Apache License 2.0',
          id: 'Apache-2.0',
          text: 'Licensed under Apache.',
          used_by: [{ crate: { name: 'tokio', version: '1.0.0' } }],
        },
      ],
    });
    expect(text).toContain(SECTIONS.rust);
    expect(text).toContain(`${rule}\nMIT License (MIT)\n${rule}`);
    expect(text).toContain('Used by: libc 0.2.0, serde 1.0.0');
    expect(text).toContain('Permission is granted.');
    expect(text).toContain(`${rule}\nApache License 2.0 (Apache-2.0)\n${rule}`);
    expect(text).toContain('Used by: tokio 1.0.0');
    expect(text.indexOf('MIT License (MIT)')).toBeLessThan(text.indexOf('Apache License 2.0'));
  });

  it('throws when a license has no text', () => {
    expect(() =>
      rustSection({
        licenses: [{ name: 'MIT License', id: 'MIT', text: '', used_by: [] }],
      })
    ).toThrow(/MIT License/);
  });
});

describe('crateFilesSection', () => {
  let dir;

  afterEach(() => {
    if (dir) fs.rmSync(dir, { recursive: true, force: true });
  });

  it('includes a crate that is in the graph and skips one that is not', () => {
    dir = tempDir();
    fs.mkdirSync(path.join(dir, 'src', 'zlib'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'src', 'zlib', 'LICENSE'), 'zlib license text');
    const text = crateFilesSection([
      { name: 'libz-sys', version: '1.2.3', dir },
      { name: 'serde', version: '1.0.0', dir },
    ]);
    expect(text).toContain(SECTIONS.crateFiles);
    expect(text).toContain(`${rule}\nzlib, in the libz-sys 1.2.3 crate\n${rule}`);
    expect(text).toContain('zlib license text');
    expect(text).not.toContain('UnRAR');
    expect(text).not.toContain('OpenSSL');
  });

  it('takes only the opening comment of a source file', () => {
    dir = tempDir();
    fs.mkdirSync(path.join(dir, 'libgit2', 'deps', 'xdiff'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'libgit2', 'COPYING'), 'libgit2 license text');
    fs.mkdirSync(path.join(dir, 'libgit2', 'deps', 'pcre'));
    fs.writeFileSync(path.join(dir, 'libgit2', 'deps', 'pcre', 'LICENCE'), 'pcre license text');
    fs.writeFileSync(
      path.join(dir, 'libgit2', 'deps', 'xdiff', 'xhistogram.c'),
      '/*\n * Copyright (C) 2010, Google Inc.\n */\n\n#include "xinclude.h"\n/* later */\n'
    );
    const text = crateFilesSection([{ name: 'libgit2-sys', version: '0.18.5', dir }]);
    expect(text).toContain('Copyright (C) 2010, Google Inc.');
    expect(text).not.toContain('#include');
    expect(text).not.toContain('later');
  });

  it('throws when the crate is in the graph and its license file is missing', () => {
    dir = tempDir();
    expect(() => crateFilesSection([{ name: 'unrar_sys', version: '0.5.8', dir }])).toThrow(
      /license\.txt/
    );
  });
});

describe('npmSection', () => {
  let root;

  afterEach(() => {
    if (root) fs.rmSync(root, { recursive: true, force: true });
  });

  function lockWith(packages) {
    return { packages: { '': { name: 'marlin' }, ...packages } };
  }

  function addPackage(location, files) {
    const folder = path.join(root, location);
    fs.mkdirSync(folder, { recursive: true });
    for (const [name, text] of Object.entries(files)) {
      fs.writeFileSync(path.join(folder, name), text);
    }
  }

  // The section always appends the build tools, so a lock that omits one is a problem.
  function withBuildTools(packages) {
    addPackage('node_modules/vite', { 'LICENSE.md': 'vite license text' });
    addPackage('node_modules/rolldown', {
      LICENSE: 'rolldown license text',
      'THIRD-PARTY-LICENSE': 'texts of what rolldown is built from',
    });
    return lockWith({
      'node_modules/vite': { version: '8.3.8', license: 'MIT', dev: true },
      'node_modules/rolldown': { version: '1.2.19', license: 'MIT', dev: true },
      ...packages,
    });
  }

  it('includes a package license file and skips a dev package', () => {
    root = tempDir();
    addPackage('node_modules/left-pad', { LICENSE: 'left-pad license' });
    addPackage('node_modules/only-dev', { LICENSE: 'dev only license' });
    const text = npmSection(
      withBuildTools({
        'node_modules/left-pad': { version: '1.0.0', license: 'MIT' },
        'node_modules/only-dev': { version: '2.0.0', license: 'GPL-3.0-only', dev: true },
      }),
      root,
      new Set(),
      ''
    );
    expect(text).toContain(SECTIONS.npm);
    expect(text).toContain(`${rule}\nleft-pad 1.0.0\n${rule}`);
    expect(text).toContain('left-pad license');
    expect(text).not.toContain('dev only license');
    expect(text).not.toContain('only-dev');
  });

  it("throws when one of a package's license files is empty", () => {
    root = tempDir();
    addPackage('node_modules/half', { 'LICENSE-APACHE': 'apache text', 'LICENSE-MIT': '\n' });
    expect(() =>
      npmSection(
        withBuildTools({ 'node_modules/half': { version: '1.0.0', license: 'MIT OR Apache-2.0' } }),
        root,
        new Set(),
        ''
      )
    ).toThrow(/half@1\.0\.0 has an empty license file/);
  });

  it('does not count a .spdx file as a license text', () => {
    root = tempDir();
    addPackage('node_modules/spdx-only', { 'LICENSE.spdx': 'spdx text' });
    expect(() =>
      npmSection(
        lockWith({ 'node_modules/spdx-only': { version: '1.0.0', license: 'MIT' } }),
        root,
        new Set(),
        ''
      )
    ).toThrow(/spdx-only/);
  });

  it('points a package at its Rust crate, and throws when that crate is absent', () => {
    root = tempDir();
    addPackage('node_modules/@tauri-apps/plugin-fs', {});
    const lock = withBuildTools({
      'node_modules/@tauri-apps/plugin-fs': { version: '2.7.1', license: 'MIT' },
    });
    const text = npmSection(lock, root, new Set(['tauri-plugin-fs']), '');
    expect(text).toContain(
      'This is the JavaScript half of the tauri-plugin-fs crate. Its license text is under "Rust crates" above.'
    );
    expect(() => npmSection(lock, root, new Set(), '')).toThrow(/tauri-plugin-fs/);
  });

  it('points a package at a notice, and throws when that heading is absent', () => {
    root = tempDir();
    addPackage('node_modules/@iconify-json/vscode-icons', {});
    const lock = withBuildTools({
      'node_modules/@iconify-json/vscode-icons': { version: '1.2.98' },
    });
    const text = npmSection(
      lock,
      root,
      new Set(),
      '# Notices\r\n\r\n### File-type icons\r\n\r\nIcons.\r\n'
    );
    expect(text).toContain('See "File-type icons" in THIRD-PARTY-NOTICES.md beside this file.');
    expect(() => npmSection(lock, root, new Set(), '# Notices\n\n### Something else\n')).toThrow(
      /File-type icons/
    );
  });

  it('includes the build tools though the lock marks them dev', () => {
    root = tempDir();
    const text = npmSection(withBuildTools({}), root, new Set(), '');
    expect(text).toContain(`${rule}\nvite 8.3.8\n${rule}`);
    expect(text).toContain('vite license text');
    expect(text).toContain(`${rule}\nrolldown 1.2.19\n${rule}`);
    expect(text).toContain('rolldown license text');
    expect(text).toContain('texts of what rolldown is built from');
  });

  it('throws when a package has no license file and no entry', () => {
    root = tempDir();
    addPackage('node_modules/mystery', {});
    expect(() =>
      npmSection(
        lockWith({ 'node_modules/mystery': { version: '0.1.0', license: 'MIT' } }),
        root,
        new Set(),
        ''
      )
    ).toThrow(
      /mystery@0\.1\.0[\s\S]*Add the package to TEXT_ELSEWHERE in scripts\/generate-license-texts\.mjs, or run npm ci\./
    );
  });
});
