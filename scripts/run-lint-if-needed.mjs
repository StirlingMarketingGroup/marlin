#!/usr/bin/env node
import { execSync, spawnSync } from 'node:child_process';

const lintExtensions = ['.ts', '.tsx', '.js', '.jsx', '.cjs', '.mjs'];
const lintConfigFiles = [
  'eslint.config.cjs',
  'tsconfig.json',
  'tsconfig.node.json',
  'package.json',
  'package-lock.json',
];

function getUpstreamRef() {
  if (process.env.GITHUB_BASE_REF) {
    return `origin/${process.env.GITHUB_BASE_REF}`;
  }
  try {
    return execSync('git rev-parse --abbrev-ref --symbolic-full-name @{u}', {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim();
  } catch {
    // fall back to origin HEAD
    try {
      const ref = execSync('git symbolic-ref refs/remotes/origin/HEAD', {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim();
      return ref.replace('refs/remotes/', '');
    } catch {
      return null;
    }
  }
}

function getChangedFiles(baseRef) {
  try {
    const diffTarget = baseRef ? `${baseRef}...HEAD` : 'HEAD^';
    const output = execSync(`git diff --name-only ${diffTarget}`, {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    });
    return output
      .split('\n')
      .map((line) => line.trim())
      .filter(Boolean);
  } catch {
    // As a fallback (e.g., initial commit), run lint to be safe
    return null;
  }
}

function isLintRelevant(file) {
  if (lintConfigFiles.some((candidate) => file.endsWith(candidate))) {
    return true;
  }
  return lintExtensions.some((ext) => file.endsWith(ext));
}

function runLint() {
  const result = spawnSync('npm', ['run', 'lint'], {
    stdio: 'inherit',
    shell: process.platform === 'win32',
  });
  process.exit(result.status ?? 0);
}

const upstream = getUpstreamRef();
const changedFiles = getChangedFiles(upstream);

if (!changedFiles) {
  console.log('Unable to determine changed files – running lint to be safe.');
  runLint();
}

if (changedFiles.length === 0) {
  console.log('No file changes detected – skipping lint.');
  process.exit(0);
}

const lintNeeded = changedFiles.some(isLintRelevant);

if (!lintNeeded) {
  console.log('Skipping lint – no relevant file changes detected.');
  process.exit(0);
}

console.log('Running lint – relevant file changes detected.');
runLint();
