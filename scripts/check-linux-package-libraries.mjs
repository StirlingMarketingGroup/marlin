// CI runs this on a built bundle:
// `node scripts/check-linux-package-libraries.mjs <bundle folder> <package kind>...`.
// Each kind is `deb` or `rpm`. The check fails when a program in `usr/bin` of a
// package needs a library that neither the build machine nor the package's rpath
// folder holds. The workflow names the kinds the build should make, so a missing
// rpm fails here too.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const bundle = process.argv[2];
const kinds = process.argv.slice(3);
if (kinds.length === 0) {
  throw new Error('Name at least one package kind: deb or rpm.');
}

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'libs-'));
process.on('exit', () => {
  fs.rmSync(tmp, { recursive: true, force: true });
});

const problems = [];
for (const kind of kinds) {
  if (kind !== 'deb' && kind !== 'rpm') {
    throw new Error(`Unknown package kind "${kind}". Use deb or rpm.`);
  }
  // The workflow empties the bundle folder before the build, so the one package is this build's.
  const names = fs.readdirSync(path.join(bundle, kind)).filter((name) => name.endsWith(`.${kind}`));
  if (names.length !== 1) {
    throw new Error(`Expected one .${kind} file in ${bundle}/${kind}, found ${names.length}.`);
  }

  const root = path.join(tmp, kind);
  fs.mkdirSync(root);
  if (kind === 'deb') {
    execFileSync('dpkg-deb', ['-x', path.join(bundle, kind, names[0]), root]);
  } else {
    execFileSync('bsdtar', ['-xf', path.join(bundle, kind, names[0]), '-C', root]);
  }

  for (const name of fs.readdirSync(path.join(root, 'usr', 'bin'))) {
    const program = path.join(root, 'usr', 'bin', name);
    // build.rs writes an empty placeholder for a helper that nothing built.
    if (fs.statSync(program).size === 0) {
      continue;
    }

    // The rpath names the folder a package installs to (`/usr/lib/marlin`), and
    // nothing is installed here. Point the loader at the same folder inside the
    // extracted package. System libraries come from the build machine, which has
    // every package the build needs.
    const installFolders = execFileSync('patchelf', ['--print-rpath', program], {
      encoding: 'utf8',
    })
      .trim()
      .split(':')
      .filter((folder) => folder.startsWith('/'));
    const ldd = execFileSync('ldd', [program], {
      encoding: 'utf8',
      env: {
        ...process.env,
        LD_LIBRARY_PATH: installFolders.map((folder) => root + folder).join(':'),
      },
    });

    for (const line of ldd.split('\n')) {
      const found = line.split(' => ')[1];
      // LD_LIBRARY_PATH is searched before the rpath, so a library found in the
      // real install folder came from a Marlin installed on the build machine.
      if (
        line.includes('not found') ||
        installFolders.some((folder) => found?.startsWith(`${folder}/`))
      ) {
        problems.push(`${names[0]}: usr/bin/${name}: ${line.trim()}`);
      }
    }
  }
}

if (problems.length > 0) {
  throw new Error(`A package lacks a library:\n${problems.join('\n')}`);
}
console.log(`Every program in the ${kinds.join(' and ')} finds its libraries.`);
