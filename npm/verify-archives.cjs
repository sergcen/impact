#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const directory = path.resolve(process.argv[2] || path.join(root, 'dist'));
const manifest = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8'));

function archiveName(packageName) {
  const unscoped = packageName.replace(/^@/, '').replaceAll('/', '-');
  return `${unscoped}-${manifest.version}.tgz`;
}

const expected = [manifest.name, ...Object.keys(manifest.optionalDependencies ?? {})]
  .map(archiveName)
  .sort();
const actual = fs
  .readdirSync(directory)
  .filter((file) => file.endsWith('.tgz'))
  .sort();

if (JSON.stringify(actual) !== JSON.stringify(expected)) {
  throw new Error(
    `Unexpected npm archive set.\nExpected: ${expected.join(', ')}\nActual: ${actual.join(', ')}`,
  );
}

console.log(`Verified ${actual.length} npm archives for version ${manifest.version}`);
