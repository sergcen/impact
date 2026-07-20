#!/usr/bin/env node
'use strict';

const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const HELP_TEXT = `Usage:
  search.cjs --root <repository> --file <path> [--file <path>] [options]
  search.cjs --root <repository> --affected [--base <ref>] [options]

Options:
  --file <path>       Query direct and transitive dependents; repeatable
  --specifier <name>  Restrict traversal to a binding; repeatable
  --direct            Return immediate importers only
  --affected          Query projects affected by Git changes
  --base <ref>        Comparison ref for --affected
  --config <path>     Use an existing Monorepa JSON/JSONC configuration
  --no-cache          Disable persistent cache reads and writes
  --rebuild-cache     Create a new cache generation
  --strict-cache      Force a complete graph rebuild
  --trust-cache       Skip automatic working-tree validation
  --no-download       Do not fall back to npm when the CLI is absent
  --compact           Print compact JSON instead of indented JSON
  -h, --help          Show this help`;

function fail(message) {
  throw new Error(message);
}

function readValue(argv, index, option) {
  const argument = argv[index];
  const inline = argument.startsWith(`${option}=`) ? argument.slice(option.length + 1) : null;
  if (inline !== null) return [inline, index];
  if (index + 1 >= argv.length) fail(`${option} requires a value`);
  return [argv[index + 1], index + 1];
}

function parseArgs(argv) {
  const options = {
    affected: false,
    compact: false,
    direct: false,
    files: [],
    noDownload: false,
    passthrough: [],
    root: process.cwd(),
    specifiers: [],
  };

  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '-h' || argument === '--help') {
      console.log(HELP_TEXT);
      return null;
    }
    if (argument === '--affected') options.affected = true;
    else if (argument === '--compact') options.compact = true;
    else if (argument === '--direct') options.direct = true;
    else if (argument === '--no-download') options.noDownload = true;
    else if (
      ['--no-cache', '--rebuild-cache', '--strict-cache', '--trust-cache'].includes(argument)
    ) {
      options.passthrough.push(argument);
    } else if (argument === '--root' || argument.startsWith('--root=')) {
      [options.root, index] = readValue(argv, index, '--root');
    } else if (argument === '--file' || argument.startsWith('--file=')) {
      let value;
      [value, index] = readValue(argv, index, '--file');
      options.files.push(value);
    } else if (argument === '--specifier' || argument.startsWith('--specifier=')) {
      let value;
      [value, index] = readValue(argv, index, '--specifier');
      options.specifiers.push(value);
    } else if (argument === '--base' || argument.startsWith('--base=')) {
      [options.base, index] = readValue(argv, index, '--base');
      options.affected = true;
    } else if (argument === '--config' || argument.startsWith('--config=')) {
      [options.config, index] = readValue(argv, index, '--config');
    } else {
      fail(`unknown option: ${argument}`);
    }
  }

  if (options.affected && options.files.length > 0) {
    fail('--affected cannot be combined with --file');
  }
  if (!options.affected && options.files.length === 0) {
    fail('provide --file <path> or --affected');
  }
  if (options.direct && options.affected) fail('--direct requires --file');
  if (options.specifiers.length > 0 && options.affected) fail('--specifier requires --file');
  return options;
}

function readJson(file) {
  try {
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  } catch {
    return null;
  }
}

function workspacePatterns(root) {
  const manifest = readJson(path.join(root, 'package.json'));
  const workspaces = manifest?.workspaces;
  if (Array.isArray(workspaces)) return workspaces.filter((value) => typeof value === 'string');
  if (Array.isArray(workspaces?.packages)) {
    return workspaces.packages.filter((value) => typeof value === 'string');
  }
  const lerna = readJson(path.join(root, 'lerna.json'));
  if (Array.isArray(lerna?.packages)) {
    return lerna.packages.filter((value) => typeof value === 'string');
  }
  return [];
}

function hasAutomaticConfig(root) {
  return ['affected.config.json', 'affected.config.jsonc', 'pnpm-workspace.yaml'].some((name) =>
    fs.existsSync(path.join(root, name)),
  );
}

function temporaryConfig(root, requested) {
  if (requested) {
    const resolved = path.resolve(root, requested);
    if (!fs.existsSync(resolved)) fail(`configuration does not exist: ${resolved}`);
    return { path: resolved };
  }
  if (hasAutomaticConfig(root)) return {};

  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'monorepa-skill-'));
  const config = path.join(directory, 'affected.config.json');
  fs.writeFileSync(config, `${JSON.stringify({ workspacePatterns: workspacePatterns(root) }, null, 2)}\n`);
  return { directory, path: config };
}

function gitOutput(root, args) {
  const result = spawnSync('git', args, { cwd: root, encoding: 'utf8' });
  return result.status === 0 ? result.stdout.trim() : '';
}

function defaultBase(root) {
  const remoteHead = gitOutput(root, ['symbolic-ref', '--quiet', '--short', 'refs/remotes/origin/HEAD']);
  if (remoteHead) return remoteHead;
  for (const candidate of ['origin/main', 'origin/master']) {
    if (gitOutput(root, ['rev-parse', '--verify', '--quiet', candidate])) return candidate;
  }
  return 'HEAD';
}

function queryArgs(options, config) {
  const args = [];
  if (options.affected) {
    args.push('affected', '--base', options.base || defaultBase(options.root));
  } else {
    args.push('dependents', ...options.files);
    for (const specifier of options.specifiers) args.push('--specifier', specifier);
    if (options.direct) args.push('--direct');
  }
  if (config) args.push('--config', config);
  args.push(...options.passthrough, '--json');
  return args;
}

function executableCandidates(root, noDownload) {
  const candidates = [];

  if (process.env.MONOREPA_IMPACT_BINARY) {
    candidates.push({
      command: path.resolve(root, process.env.MONOREPA_IMPACT_BINARY),
      prefix: [],
    });
  }

  const installedBinary = path.join(
    root,
    'node_modules',
    '@monorepa',
    'impact',
    'npm',
    'monorepa-impact.exe',
  );
  if (fs.existsSync(installedBinary)) {
    candidates.push({ command: installedBinary, prefix: [] });
  }

  if (process.platform !== 'win32') {
    const local = path.join(root, 'node_modules', '.bin', 'monorepa-impact');
    if (fs.existsSync(local)) candidates.push({ command: local, prefix: [] });
  }

  const sourceRoot = path.resolve(__dirname, '..', '..', '..');
  const sourceBinary = process.platform === 'win32'
    ? path.join(sourceRoot, 'target', 'release', 'monorepa-impact.exe')
    : path.join(sourceRoot, 'bin', 'monorepa-impact');
  if (fs.existsSync(sourceBinary)) candidates.push({ command: sourceBinary, prefix: [] });

  candidates.push({ command: 'monorepa-impact', prefix: [] });
  if (!noDownload && process.platform !== 'win32') {
    candidates.push({
      command: 'npx',
      prefix: ['--yes', '--package', '@monorepa/impact@1', 'monorepa-impact'],
      download: true,
    });
  }
  return candidates;
}

function execute(root, args, noDownload) {
  for (const candidate of executableCandidates(root, noDownload)) {
    const result = spawnSync(candidate.command, [...candidate.prefix, ...args], {
      cwd: root,
      encoding: 'utf8',
      env: process.env,
      maxBuffer: 64 * 1024 * 1024,
    });
    if (result.error?.code === 'ENOENT') continue;
    if (result.error) fail(`cannot start ${candidate.command}: ${result.error.message}`);
    if (result.status !== 0) {
      const detail = (result.stderr || result.stdout).trim();
      const source = candidate.download ? '@monorepa/impact@1 through npm' : candidate.command;
      fail(`${source} failed${detail ? `:\n${detail}` : ''}`);
    }
    return result.stdout;
  }
  const downloadHint = noDownload || process.platform === 'win32'
    ? 'install @monorepa/impact in the repository'
    : 'install @monorepa/impact or rerun without --no-download';
  fail(`monorepa-impact is unavailable; ${downloadHint}`);
}

function main() {
  const options = parseArgs(process.argv.slice(2));
  if (!options) return;
  options.root = path.resolve(options.root);
  if (!fs.statSync(options.root, { throwIfNoEntry: false })?.isDirectory()) {
    fail(`repository root is not a directory: ${options.root}`);
  }

  const temporary = temporaryConfig(options.root, options.config);
  try {
    const stdout = execute(options.root, queryArgs(options, temporary.path), options.noDownload);
    let value;
    try {
      value = JSON.parse(stdout);
    } catch {
      fail(`CLI returned invalid JSON:\n${stdout.trim()}`);
    }
    process.stdout.write(`${JSON.stringify(value, null, options.compact ? 0 : 2)}\n`);
  } finally {
    if (temporary.directory) fs.rmSync(temporary.directory, { recursive: true, force: true });
  }
}

try {
  main();
} catch (error) {
  console.error(`monorepa-find-dependencies: ${error.message}`);
  process.exitCode = 2;
}
