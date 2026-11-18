// CI runs this on a built bundle:
// `node scripts/check-bundled-licenses.mjs <bundle folder> <kind>...`.
// Each kind is `app`, `deb`, `rpm`, or `appimage`. The check fails when an
// installer is missing one of the six license files, or when the generated
// license text is missing a section. The workflow names the kinds the build
// should make, so a missing installer fails here too.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { SECTIONS } from './generate-license-texts.mjs';

const FILES = [
  'LICENSE',
  'THIRD-PARTY-NOTICES.md',
  'GPL-3.0.txt',
  'libzpl.txt',
  'THIRD-PARTY-LICENSES.txt',
  'rust-standard-library.html',
];

// How to unpack each kind of package into `root`. Each returns the folder that now holds the package's files.
const EXTRACT = {
  deb: (packed, root) => {
    execFileSync('dpkg-deb', ['-x', packed, root]);
    return root;
  },
  rpm: (packed, root) => {
    execFileSync('bsdtar', ['-xf', packed, '-C', root]);
    return root;
  },
  // An AppImage unpacks itself, into `squashfs-root` in the working folder.
  appimage: (packed, root) => {
    execFileSync(packed, ['--appimage-extract', 'usr/lib/Marlin/licenses/*'], { cwd: root });
    return path.join(root, 'squashfs-root');
  },
};

const bundle = path.resolve(process.argv[2]);
const kinds = process.argv.slice(3);
if (kinds.length === 0) {
  throw new Error('Name at least one package kind: app, deb, rpm, or appimage.');
}

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'licenses-'));
process.on('exit', () => {
  fs.rmSync(tmp, { recursive: true, force: true });
});

const problems = [];
for (const kind of kinds) {
  let licenses;
  if (kind === 'app') {
    licenses = path.join(bundle, 'macos', 'Marlin.app', 'Contents', 'Resources', 'licenses');
  } else if (EXTRACT[kind]) {
    // The workflow empties the Linux bundle folder before the build, so the one package is this build's.
    const names = fs
      .readdirSync(path.join(bundle, kind))
      .filter((name) => name.toLowerCase().endsWith(`.${kind}`));
    if (names.length !== 1) {
      throw new Error(`Expected one .${kind} file in ${bundle}/${kind}, found ${names.length}.`);
    }
    const root = path.join(tmp, kind);
    fs.mkdirSync(root);
    const unpacked = EXTRACT[kind](path.join(bundle, kind, names[0]), root);
    licenses = path.join(unpacked, 'usr', 'lib', 'Marlin', 'licenses');
  } else {
    throw new Error(`Unknown package kind "${kind}". Use app, deb, rpm, or appimage.`);
  }

  for (const name of FILES) {
    const file = path.join(licenses, name);
    if (!fs.existsSync(file)) {
      problems.push(`${kind}: licenses/${name} is missing`);
      continue;
    }
    if (fs.statSync(file).size === 0) {
      problems.push(`${kind}: licenses/${name} is empty`);
      continue;
    }
    if (name === 'THIRD-PARTY-LICENSES.txt') {
      const text = fs.readFileSync(file, 'utf8');
      for (const title of Object.values(SECTIONS)) {
        // Alone on a line: the npm section also names "Rust crates" inside a sentence.
        if (!text.includes(`\n${title}\n`)) {
          problems.push(`${kind}: licenses/${name} has no "${title}" section`);
        }
      }
    }
  }
}

if (problems.length > 0) {
  throw new Error(problems.join('\n'));
}
console.log(`Every installer holds the license texts: ${kinds.join(', ')}.`);
