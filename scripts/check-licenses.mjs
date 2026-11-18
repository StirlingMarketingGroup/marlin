import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

// These packages state no license in package.json. Each entry says where the license is.
const UNSTATED = {
  // LICENSE_MIT and LICENSE_APACHE-2.0 in https://github.com/crabnebula-dev/drag-rs
  '@crabnebula/tauri-plugin-drag': 'Apache-2.0 OR MIT',
};

// These crates carry code that is not open source. Each entry says where its license is.
// WinFsp's license exception does not cover software linked with such code, so a Windows
// build must link none of them (docs/decisions/0001-mount-architecture.md, risk 4).
const NOT_OPEN_SOURCE = [
  // vendor/unrar/license.txt in unrar_sys
  'unrar',
  'unrar_sys',
];

/**
 * Whether accepted licenses alone can meet an SPDX expression such as
 * "(MIT OR GPL-3.0-only) AND Zlib". OR needs one side, AND needs both.
 * A license with an exception ("X WITH Y") is never accepted: a person must read it.
 * A malformed expression is never accepted.
 */
export function isAccepted(expression, accepted) {
  const tokens = expression.match(/[()]|[^\s()]+/g) ?? [];
  // SPDX license names match in any letter case.
  const names = accepted.map((license) => license.toLowerCase());
  let next = 0;
  const choice = () => {
    let met = both();
    while (tokens[next] === 'OR') {
      next += 1;
      met = both() || met;
    }
    return met;
  };
  const both = () => {
    let met = one();
    while (tokens[next] === 'AND') {
      next += 1;
      met = one() && met;
    }
    return met;
  };
  const one = () => {
    const token = tokens[next];
    next += 1;
    if (token === '(') {
      const met = choice();
      if (tokens[next] !== ')') throw new SyntaxError(expression);
      next += 1;
      return met;
    }
    if (token === undefined || [')', 'AND', 'OR', 'WITH'].includes(token)) {
      throw new SyntaxError(expression);
    }
    if (tokens[next] === 'WITH') {
      next += 2;
      return false;
    }
    return names.includes(token.toLowerCase());
  };
  try {
    return choice() && next === tokens.length;
  } catch {
    return false;
  }
}

/**
 * Every npm package that can ship in the frontend bundle: its name, version,
 * lockfile location, and license. A dev dependency is not one of them.
 */
export function shippingPackages(lock) {
  const packages = [];
  for (const [location, entry] of Object.entries(lock.packages)) {
    if (location === '' || entry.dev) continue;
    const name = location.slice(location.lastIndexOf('node_modules/') + 'node_modules/'.length);
    packages.push({
      name,
      version: entry.version,
      location,
      license: entry.license ?? UNSTATED[name],
    });
  }
  return packages;
}

/**
 * Returns one line for each package that can ship in the frontend bundle and
 * states a license that is not accepted, or states none.
 */
export function unacceptedPackages(lock, accepted) {
  const problems = [];
  for (const pkg of shippingPackages(lock)) {
    if (!pkg.license) {
      problems.push(`${pkg.name}@${pkg.version} states no license`);
    } else if (!isAccepted(pkg.license, accepted)) {
      problems.push(`${pkg.name}@${pkg.version} is under ${pkg.license}`);
    }
  }
  return problems;
}

// Node resolves symlinks in import.meta.url but not in argv.
if (fs.realpathSync(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const root = new URL('..', import.meta.url);
  const srcTauri = fileURLToPath(new URL('src-tauri', root));

  // The list is the `accepted` array in about.toml, so Rust and npm share one list.
  const about = fs.readFileSync(new URL('src-tauri/about.toml', root), 'utf8');
  const accepted = [...about.match(/^accepted = \[(.*?)^\]/ms)[1].matchAll(/^\s*"([^"]+)"/gm)].map(
    (match) => match[1]
  );
  const lock = JSON.parse(fs.readFileSync(new URL('package-lock.json', root), 'utf8'));
  const problems = unacceptedPackages(lock, accepted);
  if (problems.length > 0) {
    console.error('These npm packages have a license that src-tauri/about.toml does not accept:');
    for (const problem of problems) console.error(`  ${problem}`);
    console.error('See docs/licenses.md.');
    process.exit(1);
  }

  // libzpl.txt is kept by hand, so its first line has to name the zpl-rs we build.
  const libzplVersion = fs
    .readFileSync(new URL('src-tauri/licenses/libzpl.txt', root), 'utf8')
    .split(/\r?\n/, 1)[0]
    .replace(/^libzpl /, '');
  const zpl = fs
    .readFileSync(new URL('src-tauri/Cargo.lock', root), 'utf8')
    .match(/name = "zpl-rs"\r?\nversion = "([^"]+)"/);
  if (libzplVersion !== zpl?.[1]) {
    console.error(
      `src-tauri/licenses/libzpl.txt holds the license texts of libzpl ${libzplVersion}, and Cargo.lock has zpl-rs ${zpl?.[1]}. See docs/licenses.md.`
    );
    process.exit(1);
  }

  // cargo-about names each crate whose license is not accepted, then fails.
  try {
    execFileSync(
      'cargo',
      ['about', 'generate', '--workspace', '--locked', '--fail', '--format', 'json'],
      { cwd: srcTauri, stdio: ['ignore', 'ignore', 'inherit'] }
    );
  } catch {
    console.error(
      'The Rust crates did not pass the license check. See docs/licenses.md, which also says how to install cargo-about.'
    );
    process.exit(1);
  }

  // These crates state an accepted license, so only their names can keep them out.
  const windowsCrates = execFileSync(
    'cargo',
    [
      'tree',
      '--locked',
      '--workspace',
      '-e',
      'normal',
      '--prefix',
      'none',
      '--target',
      'x86_64-pc-windows-msvc',
    ],
    { cwd: srcTauri, encoding: 'utf8' }
  );
  const linked = NOT_OPEN_SOURCE.filter((crate) =>
    windowsCrates.split('\n').some((line) => line.startsWith(`${crate} `))
  );
  if (linked.length > 0) {
    console.error(
      `A Windows build links code that is not open source: ${linked.join(', ')}. See docs/licenses.md.`
    );
    process.exit(1);
  }
}
