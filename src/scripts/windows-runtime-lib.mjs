import { createHash } from 'node:crypto';
import { access, readFile, readdir } from 'node:fs/promises';
import path from 'node:path';

export const WINDOWS_RUNTIME_SPEC = Object.freeze({
  schemaVersion: 1,
  architecture: 'x86_64-pc-windows-msvc',
  ffmpeg: Object.freeze({
    version: '8.1.2-21-gce3c09c101',
    release: 'autobuild-2026-06-30-13-34',
    asset: 'ffmpeg-n8.1.2-21-gce3c09c101-win64-gpl-8.1.zip',
    sha256: '682361e32c9631caec09e5d9f09077101c9ed90c14e275f62014fefa6d397990',
    url: 'https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-06-30-13-34/ffmpeg-n8.1.2-21-gce3c09c101-win64-gpl-8.1.zip',
    variant: 'win64-gpl-static',
  }),
  python: Object.freeze({
    version: '3.11.9',
    asset: 'python-3.11.9-embed-amd64.zip',
    sha256: '009d6bf7e3b2ddca3d784fa09f90fe54336d5b60f0e0f305c37f400bf83cfd3b',
    url: 'https://www.python.org/ftp/python/3.11.9/python-3.11.9-embed-amd64.zip',
  }),
});

export const REQUIRED_FFMPEG_FILES = Object.freeze([
  'ffmpeg.exe',
  'ffplay.exe',
  'ffprobe.exe',
]);

export const REQUIRED_PYTHON_FILES = Object.freeze([
  'python.exe',
  'python3.dll',
  'python311.dll',
  'python311.zip',
  'python311._pth',
  'Lib/site-packages/yaml/__init__.py',
  'Lib/site-packages/pydantic/__init__.py',
  'Lib/site-packages/jsonschema/__init__.py',
  'Lib/site-packages/PIL/__init__.py',
  'Lib/site-packages/numpy/__init__.py',
  'Lib/site-packages/requests/__init__.py',
  'Lib/site-packages/fastapi/__init__.py',
  'Lib/site-packages/uvicorn/__init__.py',
  'Lib/site-packages/watchfiles/__init__.py',
]);

export const REQUIRED_PYTHON_DISTRIBUTIONS = Object.freeze([
  'annotated-doc',
  'annotated-types',
  'anyio',
  'attrs',
  'certifi',
  'cffi',
  'charset-normalizer',
  'click',
  'colorama',
  'cryptography',
  'fastapi',
  'google-auth',
  'h11',
  'idna',
  'jsonschema',
  'jsonschema-specifications',
  'numpy',
  'pillow',
  'pyasn1',
  'pyasn1-modules',
  'pycparser',
  'pydantic',
  'pydantic-core',
  'python-dotenv',
  'pyyaml',
  'referencing',
  'requests',
  'rpds-py',
  'starlette',
  'typing-extensions',
  'typing-inspection',
  'urllib3',
  'uvicorn',
  'watchfiles',
]);

function canonicalDistributionName(name) {
  return name.trim().toLowerCase().replace(/[._]+/g, '-');
}

export function auditWindowsRequirementsLock(contents) {
  const pinned = [];
  const unpinned = [];
  for (const rawLine of contents.split(/\r?\n/u)) {
    const line = rawLine.split('#', 1)[0].trim();
    if (!line) continue;
    const match = line.match(/^([a-zA-Z0-9._-]+)==([^;\s]+)(?:\s*;.*)?$/u);
    if (!match) {
      unpinned.push(line);
      continue;
    }
    pinned.push(canonicalDistributionName(match[1]));
  }

  const counts = new Map();
  for (const name of pinned) counts.set(name, (counts.get(name) ?? 0) + 1);
  const actual = new Set(pinned);
  const expected = new Set(REQUIRED_PYTHON_DISTRIBUTIONS);
  const missing = REQUIRED_PYTHON_DISTRIBUTIONS.filter((name) => !actual.has(name));
  const unexpected = [...actual].filter((name) => !expected.has(name)).sort();
  const duplicates = [...counts]
    .filter(([, count]) => count > 1)
    .map(([name]) => name)
    .sort();

  return {
    ok: missing.length === 0 && unexpected.length === 0 && duplicates.length === 0 && unpinned.length === 0,
    missing,
    unexpected,
    duplicates,
    unpinned,
  };
}

export async function pathExists(filePath) {
  try {
    await access(filePath);
    return true;
  } catch {
    return false;
  }
}

export async function sha256File(filePath) {
  const hash = createHash('sha256');
  hash.update(await readFile(filePath));
  return hash.digest('hex');
}

export async function sha256TextFile(filePath) {
  const hash = createHash('sha256');
  hash.update(await readFile(filePath, 'utf8'));
  return hash.digest('hex');
}

export async function findFile(root, expectedName) {
  const entries = await readdir(root, { withFileTypes: true });
  for (const entry of entries) {
    const candidate = path.join(root, entry.name);
    if (entry.isFile() && entry.name.toLowerCase() === expectedName.toLowerCase()) {
      return candidate;
    }
    if (entry.isDirectory()) {
      const nested = await findFile(candidate, expectedName);
      if (nested) return nested;
    }
  }
  return null;
}

export function pythonPathFileContents() {
  return [
    'python311.zip',
    '.',
    'Lib',
    'Lib/site-packages',
    '',
    'import site',
    '',
  ].join('\r\n');
}

async function missingFiles(root, requiredFiles) {
  const checks = await Promise.all(
    requiredFiles.map(async (relative) => ({
      relative,
      exists: await pathExists(path.join(root, ...relative.split('/'))),
    })),
  );
  return checks.filter((item) => !item.exists).map((item) => item.relative);
}

export async function validateWindowsRuntime(runtimeRoot, { requireManifest = true } = {}) {
  const ffmpegRoot = path.join(runtimeRoot, 'ffmpeg');
  const pythonRoot = path.join(runtimeRoot, 'python');
  const missing = [
    ...(await missingFiles(ffmpegRoot, REQUIRED_FFMPEG_FILES)).map((file) => `ffmpeg/${file}`),
    ...(await missingFiles(pythonRoot, REQUIRED_PYTHON_FILES)).map((file) => `python/${file}`),
  ];
  const pathFile = path.join(pythonRoot, 'python311._pth');
  if (await pathExists(pathFile)) {
    const contents = await readFile(pathFile, 'utf8');
    if (!contents.includes('Lib/site-packages') || !contents.includes('import site')) {
      missing.push('python/python311._pth (site-packages is disabled)');
    }
  }
  const manifestPath = path.join(runtimeRoot, 'windows-runtime-manifest.json');
  if (requireManifest && !(await pathExists(manifestPath))) {
    missing.push('windows-runtime-manifest.json');
  }
  return {
    ok: missing.length === 0,
    missing,
    ffmpegRoot,
    pythonRoot,
    manifestPath,
  };
}

export function runtimeManifest(requirementsSha256, generatedAt = new Date().toISOString()) {
  return {
    schemaVersion: WINDOWS_RUNTIME_SPEC.schemaVersion,
    platform: 'windows',
    architecture: WINDOWS_RUNTIME_SPEC.architecture,
    generatedAt,
    offlineReady: true,
    ffmpeg: WINDOWS_RUNTIME_SPEC.ffmpeg,
    python: WINDOWS_RUNTIME_SPEC.python,
    pythonDependencies: {
      lockFile: 'builtin-plugins/openmontage/requirements-windows.lock',
      sha256: requirementsSha256,
      installPath: 'python/Lib/site-packages',
    },
    entrypoints: {
      ffmpeg: 'ffmpeg/ffmpeg.exe',
      ffprobe: 'ffmpeg/ffprobe.exe',
      ffplay: 'ffmpeg/ffplay.exe',
      python: 'python/python.exe',
    },
  };
}
