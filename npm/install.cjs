#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const path = require('node:path');
const { currentPlatformKey, platformPackages } = require('./platform.cjs');

const root = path.resolve(__dirname, '..');
const destination = path.join(__dirname, 'monorepa-impact.exe');

function resolveBinary(options = {}) {
  const environment = options.environment ?? process.env;
  if (environment.MONOREPA_IMPACT_BINARY) {
    return path.resolve(environment.MONOREPA_IMPACT_BINARY);
  }

  const platform = options.platform ?? currentPlatformKey();
  const packageName = platformPackages[platform];
  if (!packageName) {
    throw new Error(
      `Unsupported platform: ${platform}. ` +
        `Supported platforms: ${Object.keys(platformPackages).sort().join(', ')}.`,
    );
  }

  const resolveManifest = options.resolveManifest ?? require.resolve;
  let manifest;
  try {
    manifest = resolveManifest(`${packageName}/package.json`);
  } catch (error) {
    throw new Error(
      `The optional package ${packageName} is missing. ` +
        'Reinstall @monorepa/impact without disabling optional dependencies.',
      { cause: error },
    );
  }

  const executable = platform.startsWith('win32-')
    ? 'monorepa-impact.exe'
    : 'monorepa-impact';
  return path.join(path.dirname(manifest), 'bin', executable);
}

function installBinary(source, target = destination) {
  if (!fs.statSync(source).isFile()) {
    throw new Error(`Native executable is not a file: ${source}`);
  }

  fs.mkdirSync(path.dirname(target), { recursive: true });
  const temporary = `${target}.${process.pid}.tmp`;
  fs.rmSync(temporary, { force: true });
  try {
    fs.linkSync(source, temporary);
  } catch {
    fs.copyFileSync(source, temporary);
  }
  fs.chmodSync(temporary, 0o755);
  fs.rmSync(target, { force: true });
  fs.renameSync(temporary, target);
}

function main() {
  try {
    installBinary(resolveBinary());
  } catch (error) {
    if (fs.existsSync(path.join(root, 'Cargo.toml'))) {
      console.warn(`monorepa-impact: native npm binary was not installed: ${error.message}`);
      return;
    }
    console.error(`monorepa-impact: ${error.message}`);
    process.exitCode = 1;
  }
}

if (require.main === module) {
  main();
}

module.exports = { installBinary, resolveBinary };
