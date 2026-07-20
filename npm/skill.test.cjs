'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { test } = require('node:test');

const repository = path.resolve(__dirname, '..');
const wrapper = path.join(
  repository,
  'skills',
  'monorepa-find-dependencies',
  'scripts',
  'search.cjs',
);

function search(args) {
  const result = spawnSync(process.execPath, [wrapper, ...args], {
    cwd: repository,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  assert.equal(result.status, 0, result.stderr || result.stdout);
  return JSON.parse(result.stdout);
}

test('skill wrapper exposes self-contained usage help', () => {
  const result = spawnSync(process.execPath, [wrapper, '--help'], {
    cwd: repository,
    encoding: 'utf8',
  });

  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /--file <path>/);
  assert.match(result.stdout, /--specifier <name>/);
  assert.match(result.stdout, /--affected/);
});

test('skill wrapper finds direct dependents with binding-aware reasons', () => {
  const fixture = path.join(repository, 'tests', 'fixtures', 'real-monorepo');
  const result = search([
    '--root',
    fixture,
    '--file',
    'packages/contracts/src/models/user.ts',
    '--direct',
    '--no-cache',
    '--no-download',
  ]);

  assert.deepEqual(result.dependentFiles, [
    'packages/contracts/src/index.ts',
    'packages/ui/src/button.tsx',
  ]);
  assert.deepEqual(result.projects, ['@fixture/contracts', '@fixture/ui']);
  assert.equal(result.direct, true);
  assert.equal(result.reasons['packages/ui/src/button.tsx'][1].importedSpecifier, 'User');
});

test('skill wrapper restricts traversal to a requested export', () => {
  const fixture = path.join(repository, 'tests', 'fixtures', 'real-monorepo');
  const result = search([
    '--root',
    fixture,
    '--file',
    'packages/contracts/src/flags.ts',
    '--specifier',
    'featureFlag',
    '--no-cache',
    '--no-download',
  ]);

  assert.deepEqual(result.targetSpecifiers, ['featureFlag']);
  assert.deepEqual(result.dependentFiles, ['packages/contracts/src/index.ts']);
  assert.deepEqual(result.projects, ['@fixture/contracts']);
});

test('skill wrapper creates configuration for a single-package repository', (context) => {
  const fixture = fs.mkdtempSync(path.join(os.tmpdir(), 'monorepa-skill-test-'));
  context.after(() => fs.rmSync(fixture, { recursive: true, force: true }));
  fs.mkdirSync(path.join(fixture, 'src'));
  fs.writeFileSync(
    path.join(fixture, 'package.json'),
    `${JSON.stringify({ name: 'single-package', private: true }, null, 2)}\n`,
  );
  fs.writeFileSync(path.join(fixture, 'src', 'value.ts'), 'export const value = 1;\n');
  fs.writeFileSync(
    path.join(fixture, 'src', 'consumer.ts'),
    "import { value } from './value';\nconsole.log(value);\n",
  );

  const result = search([
    '--root',
    fixture,
    '--file',
    'src/value.ts',
    '--direct',
    '--no-cache',
    '--no-download',
  ]);

  assert.deepEqual(result.dependentFiles, ['src/consumer.ts']);
  assert.deepEqual(result.projects, []);
});
