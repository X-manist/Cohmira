import { spawnSync } from 'node:child_process';
import { createWriteStream } from 'node:fs';
import { cp, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { Readable } from 'node:stream';
import { finished } from 'node:stream/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  WINDOWS_RUNTIME_SPEC,
  auditWindowsRequirementsLock,
  findFile,
  pathExists,
  pythonPathFileContents,
  runtimeManifest,
  sha256File,
  sha256TextFile,
  validateWindowsRuntime,
} from './windows-runtime-lib.mjs';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const workspaceRoot = path.resolve(scriptDir, '..');
const runtimeRoot = path.join(workspaceRoot, 'src-tauri', 'runtime');
const ffmpegRoot = path.join(runtimeRoot, 'ffmpeg');
const pythonRoot = path.join(runtimeRoot, 'python');
const requirementsPath = path.join(
  workspaceRoot,
  'builtin-plugins',
  'openmontage',
  'requirements-windows.lock',
);

function parseArguments(argv) {
  const options = {
    force: false,
    cacheDir: process.env.JIUBAN_RUNTIME_CACHE
      ? path.resolve(process.env.JIUBAN_RUNTIME_CACHE)
      : path.join(workspaceRoot, '.runtime-cache', 'windows'),
  };
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === '--force') {
      options.force = true;
    } else if (argument === '--cache-dir') {
      const value = argv[index + 1];
      if (!value) throw new Error('--cache-dir requires a path');
      options.cacheDir = path.resolve(value);
      index += 1;
    } else {
      throw new Error(`Unknown argument: ${argument}`);
    }
  }
  return options;
}

function run(command, args, cwd = workspaceRoot) {
  console.log(`> ${command} ${args.join(' ')}`);
  const result = spawnSync(command, args, {
    cwd,
    env: process.env,
    stdio: 'inherit',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status ?? 'unknown'}`);
  }
}

async function download(url, destination) {
  console.log(`Downloading ${url}`);
  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok || !response.body) {
    throw new Error(`Download failed (${response.status}): ${url}`);
  }
  await mkdir(path.dirname(destination), { recursive: true });
  const temporary = `${destination}.partial`;
  await rm(temporary, { force: true });
  await finished(Readable.fromWeb(response.body).pipe(createWriteStream(temporary)));
  await rm(destination, { force: true });
  await cp(temporary, destination);
  await rm(temporary, { force: true });
}

async function cachedArchive(spec, cacheDir) {
  const archive = path.join(cacheDir, spec.asset);
  if (!(await pathExists(archive))) {
    await download(spec.url, archive);
  }
  let digest = await sha256File(archive);
  if (digest !== spec.sha256) {
    console.warn(`Cached archive checksum mismatch; downloading a clean copy: ${spec.asset}`);
    await rm(archive, { force: true });
    await download(spec.url, archive);
    digest = await sha256File(archive);
  }
  if (digest !== spec.sha256) {
    throw new Error(`SHA-256 mismatch for ${spec.asset}: expected ${spec.sha256}, received ${digest}`);
  }
  return archive;
}

function expandArchive(archive, destination) {
  run('powershell.exe', [
    '-NoLogo',
    '-NoProfile',
    '-NonInteractive',
    '-Command',
    '& { param($archive, $destination) Expand-Archive -LiteralPath $archive -DestinationPath $destination -Force }',
    archive,
    destination,
  ]);
}

function resolveUvCommand() {
  return process.env.UV_BIN || 'uv.exe';
}

async function installFfmpeg(cacheDir, temporaryRoot) {
  const archive = await cachedArchive(WINDOWS_RUNTIME_SPEC.ffmpeg, cacheDir);
  const extractRoot = path.join(temporaryRoot, 'ffmpeg-extract');
  await mkdir(extractRoot, { recursive: true });
  expandArchive(archive, extractRoot);
  const executable = await findFile(extractRoot, 'ffmpeg.exe');
  if (!executable) throw new Error('FFmpeg archive does not contain ffmpeg.exe');
  const distributionRoot = path.dirname(path.dirname(executable));
  await rm(ffmpegRoot, { recursive: true, force: true });
  await cp(distributionRoot, ffmpegRoot, { recursive: true });
}

async function installPython(cacheDir, temporaryRoot) {
  const archive = await cachedArchive(WINDOWS_RUNTIME_SPEC.python, cacheDir);
  const extractRoot = path.join(temporaryRoot, 'python-extract');
  await mkdir(extractRoot, { recursive: true });
  expandArchive(archive, extractRoot);
  await rm(pythonRoot, { recursive: true, force: true });
  await cp(extractRoot, pythonRoot, { recursive: true });
  await mkdir(path.join(pythonRoot, 'Lib', 'site-packages'), { recursive: true });
  await writeFile(path.join(pythonRoot, 'python311._pth'), pythonPathFileContents(), 'utf8');

  run(resolveUvCommand(), [
    '--cache-dir',
    path.join(cacheDir, 'uv'),
    'pip',
    'install',
    '--target',
    path.join(pythonRoot, 'Lib', 'site-packages'),
    '--python-version',
    '3.11',
    '--python-platform',
    'x86_64-pc-windows-msvc',
    '--only-binary',
    ':all:',
    '--no-deps',
    '--requirements',
    requirementsPath,
  ]);
}

async function smokeTest() {
  const python = path.join(pythonRoot, 'python.exe');
  run(python, [
    '-I',
    '-c',
    [
      'import fastapi, jsonschema, numpy, PIL, pydantic, requests, uvicorn, watchfiles, yaml',
      'import cryptography, dotenv, google.auth',
      "print('python-offline-runtime-ok')",
    ].join('; '),
  ]);
  run(path.join(ffmpegRoot, 'ffmpeg.exe'), ['-hide_banner', '-version']);
  run(path.join(ffmpegRoot, 'ffprobe.exe'), ['-hide_banner', '-version']);
}

async function main() {
  if (process.platform !== 'win32') {
    throw new Error('The Windows runtime must be prepared on Windows so wheel compatibility can be verified.');
  }
  const options = parseArguments(process.argv.slice(2));
  if (!(await pathExists(requirementsPath))) {
    throw new Error(`Missing Windows Python dependency lock: ${requirementsPath}`);
  }
  const requirementsAudit = auditWindowsRequirementsLock(await readFile(requirementsPath, 'utf8'));
  if (!requirementsAudit.ok) {
    throw new Error(`Invalid Windows Python dependency lock: ${JSON.stringify(requirementsAudit)}`);
  }
  const existing = await validateWindowsRuntime(runtimeRoot);
  if (existing.ok && !options.force) {
    console.log(`Windows offline runtime is already complete: ${runtimeRoot}`);
    await smokeTest();
    return;
  }

  await mkdir(options.cacheDir, { recursive: true });
  await mkdir(runtimeRoot, { recursive: true });
  const temporaryRoot = await mkdtemp(path.join(os.tmpdir(), 'jiuban-windows-runtime-'));
  try {
    await installFfmpeg(options.cacheDir, temporaryRoot);
    await installPython(options.cacheDir, temporaryRoot);
    const requirementsSha256 = await sha256TextFile(requirementsPath);
    await writeFile(
      path.join(runtimeRoot, 'windows-runtime-manifest.json'),
      `${JSON.stringify(runtimeManifest(requirementsSha256), null, 2)}\n`,
      'utf8',
    );
    const validation = await validateWindowsRuntime(runtimeRoot);
    if (!validation.ok) {
      throw new Error(`Prepared runtime is incomplete:\n- ${validation.missing.join('\n- ')}`);
    }
    await smokeTest();
    console.log(`Prepared complete Windows offline runtime: ${runtimeRoot}`);
  } finally {
    await rm(temporaryRoot, { recursive: true, force: true });
  }
}

await main();
