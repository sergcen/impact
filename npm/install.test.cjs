'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { test } = require('node:test');
const { installBinary, resolveBinary } = require('./install.cjs');
const { platformKey, platformPackages } = require('./platform.cjs');

test('installs the selected executable without a runtime JavaScript launcher', (context) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'monorepa-impact-install-'));
  context.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const source = path.join(directory, 'platform-binary');
  const target = path.join(directory, 'root-package', 'monorepa-impact.exe');
  const bytes = Buffer.from([0x46, 0x4f, 0x44, 0x43, 0x00, 0xff]);
  fs.writeFileSync(source, bytes, { mode: 0o755 });

  installBinary(source, target);

  assert.deepEqual(fs.readFileSync(target), bytes);
  if (process.platform !== 'win32') {
    assert.equal(fs.statSync(target).mode & 0o111, 0o111);
  }
});

test('resolves a platform package executable at install time', () => {
  const manifest = path.join('/packages', 'native', 'package.json');
  assert.equal(
    resolveBinary({
      environment: {},
      platform: 'linux-x64-gnu',
      resolveManifest(request) {
        assert.equal(request, '@monorepa/impact-linux-x64-gnu/package.json');
        return manifest;
      },
    }),
    path.join(path.dirname(manifest), 'bin', 'monorepa-impact'),
  );
  assert.equal(
    resolveBinary({
      environment: {},
      platform: 'win32-arm64-msvc',
      resolveManifest: () => manifest,
    }),
    path.join(path.dirname(manifest), 'bin', 'monorepa-impact.exe'),
  );
});

test('honors the renamed native binary override', () => {
  const configured = path.join('custom', 'monorepa-impact');
  assert.equal(
    resolveBinary({ environment: { MONOREPA_IMPACT_BINARY: configured } }),
    path.resolve(configured),
  );
});

test('selects every published OS, architecture, and libc package', () => {
  const glibc = { header: { glibcVersionRuntime: '2.35' } };
  const musl = { header: {} };
  const cases = [
    ['darwin', 'arm64', undefined, '@monorepa/impact-darwin-arm64'],
    ['darwin', 'x64', undefined, '@monorepa/impact-darwin-x64'],
    ['linux', 'arm64', glibc, '@monorepa/impact-linux-arm64-gnu'],
    ['linux', 'arm64', musl, '@monorepa/impact-linux-arm64-musl'],
    ['linux', 'x64', glibc, '@monorepa/impact-linux-x64-gnu'],
    ['linux', 'x64', musl, '@monorepa/impact-linux-x64-musl'],
    ['win32', 'arm64', undefined, '@monorepa/impact-win32-arm64-msvc'],
    ['win32', 'x64', undefined, '@monorepa/impact-win32-x64-msvc'],
  ];

  for (const [platform, architecture, report, expected] of cases) {
    assert.equal(platformPackages[platformKey(platform, architecture, report)], expected);
  }
});

test('does not map an unpublished architecture', () => {
  assert.equal(platformPackages[platformKey('linux', 'ia32', { header: {} })], undefined);
});

test('reports a missing optional platform package', () => {
  assert.throws(
    () =>
      resolveBinary({
        environment: {},
        platform: 'darwin-arm64',
        resolveManifest() {
          throw new Error('missing');
        },
      }),
    /optional package @monorepa\/impact-darwin-arm64 is missing/,
  );
});
