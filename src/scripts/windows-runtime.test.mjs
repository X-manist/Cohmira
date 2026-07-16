import assert from 'node:assert/strict';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { afterEach, describe, it } from 'node:test';
import os from 'node:os';
import path from 'node:path';
import { mkdtemp, rm } from 'node:fs/promises';

import {
  REQUIRED_FFMPEG_FILES,
  REQUIRED_PYTHON_FILES,
  REQUIRED_PYTHON_DISTRIBUTIONS,
  WINDOWS_RUNTIME_SPEC,
  auditWindowsRequirementsLock,
  pythonPathFileContents,
  runtimeManifest,
  validateWindowsRuntime,
} from './windows-runtime-lib.mjs';

const requirementsPath = new URL('../builtin-plugins/openmontage/requirements-windows.lock', import.meta.url);

const temporaryDirectories = [];

afterEach(async () => {
  await Promise.all(temporaryDirectories.splice(0).map((directory) => (
    rm(directory, { recursive: true, force: true })
  )));
});

async function temporaryRuntime() {
  const root = await mkdtemp(path.join(os.tmpdir(), 'jiuban-runtime-test-'));
  temporaryDirectories.push(root);
  return root;
}

async function touch(root, relative, contents = '') {
  const target = path.join(root, ...relative.split('/'));
  await mkdir(path.dirname(target), { recursive: true });
  await writeFile(target, contents);
}

describe('Windows offline runtime contract', () => {
  it('pins immutable upstream archives and checksums', () => {
    assert.match(WINDOWS_RUNTIME_SPEC.ffmpeg.url, /autobuild-2026-06-30-13-34/);
    assert.match(WINDOWS_RUNTIME_SPEC.ffmpeg.sha256, /^[a-f0-9]{64}$/);
    assert.match(WINDOWS_RUNTIME_SPEC.python.url, /python-3\.11\.9-embed-amd64\.zip$/);
    assert.match(WINDOWS_RUNTIME_SPEC.python.sha256, /^[a-f0-9]{64}$/);
  });

  it('enables bundled site-packages in the embeddable Python path file', () => {
    const contents = pythonPathFileContents();
    assert.match(contents, /Lib\/site-packages/);
    assert.match(contents, /import site/);
  });

  it('keeps the complete Windows dependency closure exactly pinned', async () => {
    const audit = auditWindowsRequirementsLock(await readFile(requirementsPath, 'utf8'));
    assert.equal(REQUIRED_PYTHON_DISTRIBUTIONS.length, 34);
    assert.deepEqual(audit, {
      ok: true,
      missing: [],
      unexpected: [],
      duplicates: [],
      unpinned: [],
    });
  });

  it('rejects dependency locks with ranges or a missing transitive wheel', () => {
    const incomplete = REQUIRED_PYTHON_DISTRIBUTIONS
      .filter((name) => name !== 'pydantic-core')
      .map((name) => `${name}==1.0.0`)
      .concat('local-package>=1')
      .join('\n');
    const audit = auditWindowsRequirementsLock(incomplete);
    assert.equal(audit.ok, false);
    assert.deepEqual(audit.missing, ['pydantic-core']);
    assert.deepEqual(audit.unpinned, ['local-package>=1']);
  });

  it('reports every missing executable and Python package', async () => {
    const root = await temporaryRuntime();
    const result = await validateWindowsRuntime(root);
    assert.equal(result.ok, false);
    assert.equal(
      result.missing.length,
      REQUIRED_FFMPEG_FILES.length + REQUIRED_PYTHON_FILES.length + 1,
    );
    assert.ok(result.missing.includes('ffmpeg/ffmpeg.exe'));
    assert.ok(result.missing.includes('python/python.exe'));
    assert.ok(result.missing.includes('windows-runtime-manifest.json'));
  });

  it('accepts a complete self-contained layout', async () => {
    const root = await temporaryRuntime();
    await Promise.all(REQUIRED_FFMPEG_FILES.map((file) => touch(root, `ffmpeg/${file}`)));
    await Promise.all(REQUIRED_PYTHON_FILES.map((file) => (
      touch(
        root,
        `python/${file}`,
        file === 'python311._pth' ? pythonPathFileContents() : '',
      )
    )));
    await touch(
      root,
      'windows-runtime-manifest.json',
      JSON.stringify(runtimeManifest('a'.repeat(64), '2026-07-15T00:00:00.000Z')),
    );
    const result = await validateWindowsRuntime(root);
    assert.deepEqual(result.missing, []);
    assert.equal(result.ok, true);
  });
});
