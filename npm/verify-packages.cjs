#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');

const root = path.resolve(__dirname, '..');
const rootManifest = require(path.join(root, 'package.json'));
const cargoManifest = fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8');
const cargoVersion = cargoManifest.match(/\[package\][\s\S]*?\nversion\s*=\s*"([^"]+)"/)?.[1];
const cargoDescription = cargoManifest.match(
  /\[package\][\s\S]*?\ndescription\s*=\s*"([^"]+)"/,
)?.[1];
const { platformPackages } = require('./platform.cjs');
const expected = {
  'darwin-arm64': { name: '@monorepa/impact-darwin-arm64', os: 'darwin', cpu: 'arm64' },
  'darwin-x64': { name: '@monorepa/impact-darwin-x64', os: 'darwin', cpu: 'x64' },
  'linux-arm64-gnu': {
    name: '@monorepa/impact-linux-arm64-gnu',
    os: 'linux',
    cpu: 'arm64',
    libc: 'glibc',
  },
  'linux-arm64-musl': {
    name: '@monorepa/impact-linux-arm64-musl',
    os: 'linux',
    cpu: 'arm64',
    libc: 'musl',
  },
  'linux-x64-gnu': {
    name: '@monorepa/impact-linux-x64-gnu',
    os: 'linux',
    cpu: 'x64',
    libc: 'glibc',
  },
  'linux-x64-musl': {
    name: '@monorepa/impact-linux-x64-musl',
    os: 'linux',
    cpu: 'x64',
    libc: 'musl',
  },
  'win32-arm64-msvc': {
    name: '@monorepa/impact-win32-arm64-msvc',
    os: 'win32',
    cpu: 'arm64',
  },
  'win32-x64-msvc': { name: '@monorepa/impact-win32-x64-msvc', os: 'win32', cpu: 'x64' },
};

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

const projectMetadata = {
  author: 'Monorepa',
  license: 'MIT',
  repository: 'git+https://github.com/sergcen/impact.git',
  homepage: 'https://github.com/sergcen/impact#readme',
  bugs: 'https://github.com/sergcen/impact/issues',
};

function assertProjectMetadata(manifest, location) {
  assert(manifest.author === projectMetadata.author, `${location}: unexpected author`);
  assert(manifest.license === projectMetadata.license, `${location}: unexpected license`);
  assert(manifest.repository?.type === 'git', `${location}: repository must use Git`);
  assert(
    manifest.repository?.url === projectMetadata.repository,
    `${location}: unexpected repository`,
  );
  assert(manifest.homepage === projectMetadata.homepage, `${location}: unexpected homepage`);
  assert(manifest.bugs?.url === projectMetadata.bugs, `${location}: unexpected bugs URL`);
}

assert(rootManifest.name === '@monorepa/impact', 'unexpected root package name');
assertProjectMetadata(rootManifest, 'root package');
assert(fs.existsSync(path.join(root, 'LICENSE')), 'MIT license file is missing');
assert(rootManifest.packageManager === 'npm@12.0.1', 'unexpected npm toolchain version');
assert(rootManifest.private !== true, 'root package must be publishable');
assert(
  rootManifest.bin?.monorepa === 'npm/monorepa.exe',
  'root CLI entrypoint must be the installed native executable',
);
assert(Object.keys(rootManifest.bin ?? {}).length === 1, 'root package exposes an unexpected CLI');
assert(rootManifest.scripts?.postinstall === 'node npm/install.cjs', 'missing native installer');
assert(rootManifest.files?.includes('npm/install.cjs'), 'native installer is not publishable');
assert(rootManifest.files?.includes('npm/platform.cjs'), 'platform resolver is not publishable');
assert(rootManifest.files?.includes('LICENSE'), 'MIT license is not publishable');
assert(rootManifest.files?.includes('docs'), 'linked documentation is not publishable');
assert(
  rootManifest.files?.includes('npm/monorepa.exe'),
  'native executable destination is not publishable',
);
assert(rootManifest.preferUnplugged === true, 'root package must be writable during installation');
assert(!fs.existsSync(path.join(__dirname, 'cli.cjs')), 'runtime JavaScript launcher still exists');
assert(rootManifest.publishConfig?.access === 'public', 'root package must publish as public');
assert(cargoVersion === rootManifest.version, 'Cargo and npm package versions must match');
assert(
  cargoDescription === rootManifest.description,
  'Cargo and npm package descriptions must match',
);
assert(
  Object.keys(rootManifest.optionalDependencies ?? {}).length === Object.keys(expected).length,
  'root package has an unexpected platform dependency',
);
assert(
  Object.keys(platformPackages).length === Object.keys(expected).length,
  'installer has an unexpected platform mapping',
);

for (const [directory, platform] of Object.entries(expected)) {
  const manifestPath = path.join(__dirname, directory, 'package.json');
  const manifest = JSON.parse(fs.readFileSync(manifestPath, 'utf8'));
  const dependencyVersion = rootManifest.optionalDependencies?.[platform.name];
  assertProjectMetadata(manifest, directory);
  assert(
    fs.readFileSync(path.join(__dirname, directory, 'LICENSE'), 'utf8') ===
      fs.readFileSync(path.join(root, 'LICENSE'), 'utf8'),
    `${directory}: license file differs from the root license`,
  );
  assert(platformPackages[directory] === platform.name, `${directory}: installer package mismatch`);
  assert(manifest.name === platform.name, `${directory}: unexpected package name`);
  assert(manifest.version === rootManifest.version, `${directory}: version mismatch`);
  assert(dependencyVersion === rootManifest.version, `${directory}: optional dependency mismatch`);
  assert(manifest.os?.length === 1 && manifest.os[0] === platform.os, `${directory}: invalid os`);
  assert(manifest.cpu?.length === 1 && manifest.cpu[0] === platform.cpu, `${directory}: invalid cpu`);
  assert(manifest.private !== true, `${directory}: package must be publishable`);
  assert(manifest.license === rootManifest.license, `${directory}: license mismatch`);
  assert(manifest.files?.includes('bin'), `${directory}: binary directory is not publishable`);
  assert(manifest.files?.includes('LICENSE'), `${directory}: MIT license is not publishable`);
  assert(manifest.publishConfig?.access === 'public', `${directory}: package must publish as public`);
  assert(manifest.preferUnplugged === true, `${directory}: native package must be unplugged`);
  if (platform.libc) {
    assert(
      manifest.libc?.length === 1 && manifest.libc[0] === platform.libc,
      `${directory}: invalid libc`,
    );
  }
}

console.log(`npm package metadata is consistent for @monorepa/impact ${rootManifest.version}`);
