#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const directory = path.resolve(process.argv[2] || '.');
const manifestPath = path.join(directory, 'package.json');
const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
const repository = process.env.GITHUB_REPOSITORY;

if (!repository) {
  throw new Error('GITHUB_REPOSITORY is required to prepare a provenance-enabled package');
}

const expectedRepository = `git+https://github.com/${repository}.git`;
const expectedHomepage = `https://github.com/${repository}#readme`;
const expectedIssues = `https://github.com/${repository}/issues`;

if (manifest.repository?.type !== 'git' || manifest.repository?.url !== expectedRepository) {
  throw new Error(`${manifest.name}: repository metadata does not match ${repository}`);
}
if (manifest.homepage !== expectedHomepage) {
  throw new Error(`${manifest.name}: homepage metadata does not match ${repository}`);
}
if (manifest.bugs?.url !== expectedIssues) {
  throw new Error(`${manifest.name}: bugs metadata does not match ${repository}`);
}

console.log(`Prepared ${manifest.name} metadata for ${repository}`);
