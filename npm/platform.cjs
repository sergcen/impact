'use strict';

const platformPackages = Object.freeze({
  'darwin-arm64': '@monorepa/impact-darwin-arm64',
  'darwin-x64': '@monorepa/impact-darwin-x64',
  'linux-arm64-gnu': '@monorepa/impact-linux-arm64-gnu',
  'linux-arm64-musl': '@monorepa/impact-linux-arm64-musl',
  'linux-x64-gnu': '@monorepa/impact-linux-x64-gnu',
  'linux-x64-musl': '@monorepa/impact-linux-x64-musl',
  'win32-arm64-msvc': '@monorepa/impact-win32-arm64-msvc',
  'win32-x64-msvc': '@monorepa/impact-win32-x64-msvc',
});

function platformKey(platform, architecture, report) {
  if (platform === 'linux') {
    const libc = report?.header?.glibcVersionRuntime ? 'gnu' : 'musl';
    return `${platform}-${architecture}-${libc}`;
  }
  if (platform === 'win32') {
    return `${platform}-${architecture}-msvc`;
  }
  return `${platform}-${architecture}`;
}

function currentPlatformKey() {
  const report = process.platform === 'linux' ? process.report?.getReport?.() : undefined;
  return platformKey(process.platform, process.arch, report);
}

module.exports = {
  currentPlatformKey,
  platformKey,
  platformPackages,
};
