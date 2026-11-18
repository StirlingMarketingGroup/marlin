import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { shippingPackages } from './check-licenses.mjs';

// A text too many does no harm, and a missing text is the defect. The generator
// takes every crate cargo-about reports for the whole workspace, every CRATE_FILES
// crate in the target's dependency graph (build dependencies too: openssl-src is
// one), and every npm package that can ship. It does not try to work out which
// helper a given release job built.

// License files that cargo-about leaves out. Each entry names a file in a crate. Two kinds:
// - A part the crate builds in (code, fonts, data) under a license its label does not
//   name. docs/licenses.md, rule 4.
// - The crate's own license file, when cargo-about cannot match it (one file that holds
//   several licenses, or an unusual name) and prints a text with no copyright line.
// `comment` marks a source file whose license is its opening comment.
const CRATE_FILES = [
  { crate: 'unrar_sys', name: 'UnRAR', file: 'vendor/unrar/license.txt' },
  { crate: 'libgit2-sys', name: 'libgit2', file: 'libgit2/COPYING' },
  { crate: 'libgit2-sys', name: 'PCRE in libgit2', file: 'libgit2/deps/pcre/LICENCE' },
  {
    crate: 'libgit2-sys',
    name: 'xhistogram.c in libgit2',
    file: 'libgit2/deps/xdiff/xhistogram.c',
    comment: true,
  },
  { crate: 'openssl-src', name: 'OpenSSL', file: 'openssl/LICENSE.txt' },
  { crate: 'libdbus-sys', name: 'libdbus', file: 'vendor/dbus/COPYING' },
  { crate: 'bzip2-sys', name: 'bzip2', file: 'bzip2-1.0.8/LICENSE' },
  { crate: 'libz-sys', name: 'zlib', file: 'src/zlib/LICENSE' },
  { crate: 'lzma-sys', name: 'XZ Utils', file: 'xz-5.2/COPYING' },
  { crate: 'hayro-font', name: 'The Foxit fonts', file: 'assets/LICENSE_FOXIT' },
  { crate: 'hayro-cmap', name: "Adobe's CMap resources", file: 'assets/LICENSE.txt' },
  { crate: 'regex-syntax', name: 'Unicode tables', file: 'src/unicode_tables/LICENSE-UNICODE' },
  { crate: 'ring', name: 'fiat-crypto', file: 'third_party/fiat/LICENSE' },
  { crate: 'aws-lc-sys', name: 'AWS-LC', file: 'LICENSE' },
  { crate: 'chrono', name: 'chrono', file: 'LICENSE.txt' },
  { crate: 'exr', name: 'exr', file: 'LICENSE.md' },
  { crate: 'libm', name: 'libm', file: 'LICENSE.txt' },
  { crate: 'md5', name: 'md5', file: 'LICENSE.md' },
  { crate: 'siphasher', name: 'siphasher', file: 'COPYING' },
];

// These npm packages carry no license text. Each entry says where the text is.
const TEXT_ELSEWHERE = {
  '@crabnebula/tauri-plugin-drag': { crate: 'tauri-plugin-drag' },
  '@tauri-apps/plugin-dialog': { crate: 'tauri-plugin-dialog' },
  '@tauri-apps/plugin-fs': { crate: 'tauri-plugin-fs' },
  '@tauri-apps/plugin-opener': { crate: 'tauri-plugin-opener' },
  '@tauri-apps/plugin-os': { crate: 'tauri-plugin-os' },
  '@tauri-apps/plugin-shell': { crate: 'tauri-plugin-shell' },
  '@iconify-json/vscode-icons': { notice: 'File-type icons' },
};

// These are dev dependencies, and each adds a few lines of its own to the frontend
// bundle: Vite's module preload helper and Rolldown's module helpers.
const BUILD_TOOLS = ['vite', 'rolldown'];

export const SECTIONS = {
  rust: 'Rust crates',
  crateFiles: 'More license texts from Rust crates',
  npm: 'npm packages in the frontend',
};

const ROOT = new URL('..', import.meta.url);

function section(title, blocks) {
  const rule = '='.repeat(80);
  return `${rule}\n${title}\n${rule}\n\n${blocks.join('\n')}`;
}

function headed(title, body) {
  const rule = '-'.repeat(80);
  return `${rule}\n${title}\n${rule}\n\n${body.endsWith('\n') ? body : `${body}\n`}`;
}

/**
 * The "Rust crates" section of THIRD-PARTY-LICENSES.txt.
 * `about` is the parsed JSON of `cargo about generate --format json`.
 */
export function rustSection(about) {
  if (!about.licenses?.length) {
    throw new Error('cargo-about reported no licenses.');
  }
  const blocks = [];
  for (const license of about.licenses) {
    if (!license.text) {
      throw new Error(`${license.name} (${license.id}) has no license text.`);
    }
    const used = license.used_by.map((use) => `${use.crate.name} ${use.crate.version}`).join(', ');
    blocks.push(headed(`${license.name} (${license.id})`, `Used by: ${used}\n\n${license.text}`));
  }
  return section(SECTIONS.rust, blocks);
}

/**
 * The "More license texts from Rust crates" section. `packages` is the crates
 * in the build's dependency graph: `{ name, version, dir }`.
 */
export function crateFilesSection(packages) {
  const blocks = [];
  for (const entry of CRATE_FILES) {
    // A target that does not build the crate has no package for it.
    for (const pkg of packages.filter((pkg) => pkg.name === entry.crate)) {
      // A missing file fails here, and the error names its path.
      const whole = fs.readFileSync(path.join(pkg.dir, entry.file), 'utf8');
      const text = entry.comment ? (whole.match(/\/\*[\s\S]*?\*\//)?.[0] ?? '') : whole;
      if (text.trim() === '') {
        throw new Error(`${entry.file} in ${entry.crate} ${pkg.version} is empty.`);
      }
      blocks.push(headed(`${entry.name}, in the ${entry.crate} ${pkg.version} crate`, text));
    }
  }
  return section(SECTIONS.crateFiles, blocks);
}

/**
 * The "npm packages in the frontend" section. `notices` is the text of
 * THIRD-PARTY-NOTICES.md. `crates` is the set of crate names cargo-about listed.
 */
export function npmSection(lock, root, crates, notices) {
  const packages = shippingPackages(lock);
  for (const name of BUILD_TOOLS) {
    const location = `node_modules/${name}`;
    packages.push({ name, version: lock.packages[location]?.version, location });
  }

  const problems = [];
  const blocks = [];
  for (const pkg of packages) {
    const folder = path.join(root, pkg.location);
    const files = fs.existsSync(folder)
      ? fs
          .readdirSync(folder, { withFileTypes: true })
          .filter(
            (entry) =>
              // Some packages have a `licenses` folder.
              entry.isFile() &&
              /^(licen[cs]e|copying|notice|third-party-licen[cs]e)/i.test(entry.name) &&
              !entry.name.endsWith('.spdx')
          )
          .map((entry) => entry.name)
          .sort()
      : [];
    const elsewhere = TEXT_ELSEWHERE[pkg.name];
    if (files.length > 0) {
      const texts = files.map((name) => fs.readFileSync(path.join(folder, name), 'utf8'));
      if (texts.some((text) => text.trim() === '')) {
        problems.push(`${pkg.name}@${pkg.version} has an empty license file`);
      }
      blocks.push(headed(`${pkg.name} ${pkg.version}`, texts.join('\n')));
    } else if (elsewhere?.crate) {
      if (!crates.has(elsewhere.crate)) {
        problems.push(`${pkg.name}: the build has no ${elsewhere.crate} crate`);
      }
      blocks.push(
        headed(
          `${pkg.name} ${pkg.version}`,
          `This is the JavaScript half of the ${elsewhere.crate} crate. Its license text is under "Rust crates" above.`
        )
      );
    } else if (elsewhere?.notice) {
      if (!notices.split(/\r?\n/).includes(`### ${elsewhere.notice}`)) {
        problems.push(`${pkg.name}: THIRD-PARTY-NOTICES.md has no "${elsewhere.notice}" heading`);
      }
      blocks.push(
        headed(
          `${pkg.name} ${pkg.version}`,
          `See "${elsewhere.notice}" in THIRD-PARTY-NOTICES.md beside this file.`
        )
      );
    } else {
      problems.push(
        `${pkg.name}@${pkg.version} has no license text. Add the package to TEXT_ELSEWHERE in scripts/generate-license-texts.mjs, or run npm ci.`
      );
    }
  }

  if (problems.length > 0) {
    throw new Error(problems.join('\n'));
  }
  return section(SECTIONS.npm, blocks);
}

// A write that changes nothing would still give the file a new time, and the
// app's build script would run again and relink the app on every bundle build.
function writeIfChanged(file, bytes) {
  const next = Buffer.isBuffer(bytes) ? bytes : Buffer.from(bytes);
  let current;
  try {
    current = fs.readFileSync(file);
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  if (!current?.equals(next)) {
    fs.writeFileSync(file, next);
  }
}

/**
 * Writes THIRD-PARTY-LICENSES.txt and rust-standard-library.html for one
 * target triple, the host's by default. Returns the path of the text file.
 */
export function generateLicenseTexts(
  target = execFileSync('rustc', ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)[1]
) {
  try {
    execFileSync('cargo', ['about', '--version'], { stdio: 'ignore' });
  } catch {
    throw new Error('cargo-about is not installed. Run: npm run install:cargo-about');
  }

  const srcTauri = fileURLToPath(new URL('src-tauri', ROOT));
  const run = (args) =>
    execFileSync('cargo', args, {
      cwd: srcTauri,
      encoding: 'utf8',
      maxBuffer: 256 * 1024 * 1024,
      stdio: ['ignore', 'pipe', 'inherit'],
    });
  const about = JSON.parse(
    run([
      'about',
      'generate',
      '--workspace',
      '--locked',
      '--fail',
      '--format',
      'json',
      '--target',
      target,
    ])
  );
  const metadata = JSON.parse(
    run(['metadata', '--format-version', '1', '--locked', '--filter-platform', target])
  );
  const ids = new Set(metadata.resolve.nodes.map((node) => node.id));
  const packages = metadata.packages
    .filter((pkg) => ids.has(pkg.id))
    .map((pkg) => ({
      name: pkg.name,
      version: pkg.version,
      dir: path.dirname(pkg.manifest_path),
    }));

  const sysroot = execFileSync('rustc', ['--print', 'sysroot'], { encoding: 'utf8' }).trim();
  // The `rustc` component of every toolchain has this file.
  const rustHtml = fs.readFileSync(
    path.join(sysroot, 'share', 'doc', 'rust', 'COPYRIGHT-library.html')
  );

  const crates = new Set();
  for (const license of about.licenses) {
    for (const use of license.used_by) crates.add(use.crate.name);
  }
  const lock = JSON.parse(fs.readFileSync(new URL('package-lock.json', ROOT), 'utf8'));
  const notices = fs.readFileSync(new URL('THIRD-PARTY-NOTICES.md', ROOT), 'utf8');
  const intro = `License texts for the code built into this copy of Marlin (${target}).

This file is made when Marlin is built, from src-tauri/Cargo.lock and
package-lock.json. Beside it:

- LICENSE: the license of Marlin's own code.
- THIRD-PARTY-NOTICES.md: the parts whose licenses ask for more than their text.
- GPL-3.0.txt: the license of the built marlin-smb helper, in a copy that has it.
- libzpl.txt: the license texts of libzpl.
- rust-standard-library.html: the license texts of the Rust standard library,
  which is linked into each program.
`;
  const text = [
    intro,
    rustSection(about),
    crateFilesSection(packages),
    npmSection(lock, fileURLToPath(ROOT), crates, notices),
  ].join('\n');

  const textFile = path.join(srcTauri, 'licenses', 'THIRD-PARTY-LICENSES.txt');
  writeIfChanged(textFile, text);
  writeIfChanged(path.join(srcTauri, 'licenses', 'rust-standard-library.html'), rustHtml);
  return textFile;
}

// Node resolves symlinks in import.meta.url but not in argv.
if (fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const textFile = generateLicenseTexts(process.argv[2]);
  console.log(`${textFile} (${fs.statSync(textFile).size} bytes)`);
}
