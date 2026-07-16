import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import path from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const workspaceRoot = path.resolve(scriptDir, '..');

function configuredSensitiveFields(value, currentPath = '') {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) =>
      configuredSensitiveFields(item, `${currentPath}[${index}]`),
    );
  }
  if (!value || typeof value !== 'object') return [];

  return Object.entries(value).flatMap(([key, item]) => {
    const fieldPath = currentPath ? `${currentPath}.${key}` : key;
    const sensitive = /(api[_-]?key|authorization|credential|private[_-]?key|token|secret|password|cookies?)$/i.test(key);
    const configured =
      typeof item === 'string'
        ? item.trim().length > 0
        : item !== null && item !== false && item !== undefined;
    return [
      ...(sensitive && configured ? [fieldPath] : []),
      ...configuredSensitiveFields(item, fieldPath),
    ];
  });
}

test('employee bundle seeds a credential-free configuration', async () => {
  const tauriConfig = JSON.parse(
    await readFile(path.join(workspaceRoot, 'src-tauri', 'tauri.conf.json'), 'utf8'),
  );
  const resources = tauriConfig.bundle?.resources;

  assert.equal(resources?.['../config.json.example'], 'config.json');
  assert.equal(resources?.['../config.json'], undefined);

  const template = JSON.parse(
    await readFile(path.join(workspaceRoot, 'config.json.example'), 'utf8'),
  );
  assert.deepEqual(configuredSensitiveFields(template), []);
  assert.equal(template.safety?.real_confirm, '');
  for (const [key, value] of Object.entries(template.safety ?? {})) {
    if (key.startsWith('run_real_') || key === 'run_crawler_readback') {
      assert.equal(value, false, `${key} must remain disabled in the bundled seed config`);
    }
  }
});

test('every Tauri platform configuration rejects a local credential file resource', async () => {
  const tauriDir = path.join(workspaceRoot, 'src-tauri');
  const configNames = (await readdir(tauriDir)).filter(
    (name) => /^tauri(?:\..+)?\.conf\.json$/i.test(name) || /^tauri\.conf\.json$/i.test(name),
  );
  assert.ok(configNames.length > 0, 'at least one Tauri configuration must be checked');
  for (const configName of configNames) {
    const config = JSON.parse(await readFile(path.join(tauriDir, configName), 'utf8'));
    const resources = config.bundle?.resources ?? {};
    for (const source of Object.keys(resources)) {
      const normalized = source.replaceAll('\\', '/').toLowerCase();
      assert.ok(
        !normalized.endsWith('/config.json') && normalized !== 'config.json',
        `${configName} must not bundle a local config.json resource`,
      );
    }
  }
});

test('legacy Windows NSIS installer also bundles only the credential-free template', async () => {
  const installer = await readFile(
    path.join(workspaceRoot, 'src-tauri', 'installer', 'windows', 'cohmira-installer.nsi'),
    'utf8',
  );
  assert.match(installer, /!define CONFIG_FILE "\.\.\/\.\.\/\.\.\/config\.json\.example"/);
  assert.doesNotMatch(installer, /!define CONFIG_FILE "\.\.\/\.\.\/\.\.\/config\.json"/);
  assert.match(installer, /File \/oname=config\.json "\$\{CONFIG_FILE\}"/);
});
