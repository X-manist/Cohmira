import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const desktopDir = path.resolve(scriptDir, '..');
const repositoryRoot = path.resolve(desktopDir, '..', '..');
const rustRoot = path.join(repositoryRoot, 'src');
const manifestPath = path.join(rustRoot, 'Cargo.toml');

function readOption(name) {
  const index = process.argv.indexOf(name);
  if (index === -1) return undefined;
  const value = process.argv[index + 1];
  if (!value || value.startsWith('--')) {
    throw new Error(`${name} 需要一个值`);
  }
  return value;
}

if (process.argv.includes('--help')) {
  console.log(`从仓库内 Goose 源码编译老板端可使用的 goosed：

  npm run goose:build -- [--debug] [--target <rust-target>] [--features <cargo-features>]

默认构建 release + rustls-tls。可通过 GOOSE_BUILD_TARGET、
GOOSE_BUILD_FEATURES、CARGO 或 CARGO_TARGET_DIR 覆盖。构建产物保留在
src/target 中，不会复制到 Git 跟踪目录。`);
  process.exit(0);
}

if (!existsSync(manifestPath)) {
  throw new Error(`找不到 Rust workspace: ${manifestPath}`);
}

const debug = process.argv.includes('--debug');
const profile = debug ? 'debug' : 'release';
const target = readOption('--target') || String(process.env.GOOSE_BUILD_TARGET || '').trim();
const features = readOption('--features') || String(process.env.GOOSE_BUILD_FEATURES || 'rustls-tls').trim();
const cargo = String(process.env.CARGO || 'cargo').trim();
const args = [
  'build',
  '--manifest-path',
  manifestPath,
  '--package',
  'goose-server',
  '--bin',
  'goosed',
  '--no-default-features',
  '--locked',
];

if (features) args.push('--features', features);
if (!debug) args.push('--release');
if (target) args.push('--target', target);

console.log(`正在从 ${path.relative(repositoryRoot, manifestPath)} 编译 goosed (${profile})...`);
const result = spawnSync(cargo, args, {
  cwd: rustRoot,
  env: process.env,
  stdio: 'inherit',
});

if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

const configuredTargetDir = String(process.env.CARGO_TARGET_DIR || '').trim();
const targetDir = configuredTargetDir
  ? path.resolve(rustRoot, configuredTargetDir)
  : path.join(rustRoot, 'target');
const windowsBinary = target ? target.includes('windows') : process.platform === 'win32';
const executableName = windowsBinary ? 'goosed.exe' : 'goosed';
const executablePath = path.join(
  targetDir,
  ...(target ? [target] : []),
  profile,
  executableName,
);

if (!existsSync(executablePath)) {
  throw new Error(`Cargo 成功退出，但未找到 goosed: ${executablePath}`);
}

console.log(`goosed 已生成：${executablePath}`);
