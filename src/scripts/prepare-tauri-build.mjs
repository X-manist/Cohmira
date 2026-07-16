import { access, chmod, copyFile, mkdir } from 'node:fs/promises';
import { constants } from 'node:fs';
import { spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const workspaceRoot = path.resolve(scriptDir, '..');
const desktopDir = path.join(workspaceRoot, 'desktop');
const tauriDir = path.join(workspaceRoot, 'src-tauri');
const runtimeBinDir = path.join(tauriDir, 'runtime', 'bin');
const configExamplePath = path.join(workspaceRoot, 'config.json.example');

const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const cargoCommand = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
const executableSuffix = process.platform === 'win32' ? '.exe' : '';

function run(command, args, cwd) {
  console.log(`> ${command} ${args.join(' ')}`);
  const result = spawnSync(command, args, {
    cwd,
    env: process.env,
    stdio: 'inherit',
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

async function exists(filePath) {
  try {
    await access(filePath, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

if (!(await exists(configExamplePath))) {
  throw new Error(`Missing configuration template: ${configExamplePath}`);
}

if (process.platform === 'win32') {
  run(process.execPath, [path.join(scriptDir, 'prepare-windows-runtime.mjs')], workspaceRoot);
}

run(npmCommand, ['run', 'build'], desktopDir);
run(cargoCommand, [
  'build',
  '--manifest-path',
  path.join(workspaceRoot, 'Cargo.toml'),
  '--locked',
  '-p',
  'yunying-ops',
  '--bin',
  'yunying-ops-mcp',
  '--features',
  'mcp',
  '--release',
], workspaceRoot);

const helperName = `yunying-ops-mcp${executableSuffix}`;
const helperSource = path.join(workspaceRoot, 'target', 'release', helperName);
const helperDestination = path.join(runtimeBinDir, helperName);

if (!(await exists(helperSource))) {
  throw new Error(`Built operations helper is missing: ${helperSource}`);
}

await mkdir(runtimeBinDir, { recursive: true });
await copyFile(helperSource, helperDestination);
if (process.platform !== 'win32') {
  await chmod(helperDestination, 0o755);
}

console.log(`Prepared Tauri helper runtime: ${helperDestination}`);
