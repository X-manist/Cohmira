import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const ts = require('typescript') as typeof import('typescript');

type McpStoreExports = {
  getGooseMcpExtensionOverrides: () => Array<Record<string, unknown>>;
};

const __dirname = path.dirname(fileURLToPath(import.meta.url));

function loadGooseMcpExtensionOverrides(settings: Record<string, unknown>): Array<Record<string, unknown>> {
  const modulePath = path.resolve(__dirname, '..', 'mcpStore.ts');
  const source = fs.readFileSync(modulePath, 'utf8');
  const transpiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
      esModuleInterop: true,
      allowSyntheticDefaultImports: true,
    },
  }).outputText;
  const module = { exports: {} as McpStoreExports };
  const fakeDb = {
    getSettings: () => settings,
    getWorkspacePaths: () => ({ base: '/workspace/beav' }),
    saveSettings: () => undefined,
  };
  const localRequire = (specifier: string) => {
    if (specifier === '../db' || specifier === '../db.ts') {
      return fakeDb;
    }
    return require(specifier);
  };
  const processWithResources = process as typeof process & { resourcesPath?: string };
  const hadResourcesPath = Object.prototype.hasOwnProperty.call(processWithResources, 'resourcesPath');
  const previousResourcesPath = processWithResources.resourcesPath;
  const previousCwd = process.cwd();
  const desktopDir = path.resolve(__dirname, '..', '..', '..');

  try {
    process.chdir(desktopDir);
    Object.defineProperty(processWithResources, 'resourcesPath', {
      value: path.join(desktopDir, 'dist-test-resources'),
      writable: true,
      configurable: true,
    });
    const wrapper = `(function (exports, require, module, __filename, __dirname) {\n${transpiled}\n})`;
    const script = new vm.Script(wrapper, { filename: modulePath });
    const runModule = script.runInThisContext();
    runModule(module.exports, localRequire, module, modulePath, path.dirname(modulePath));
    return module.exports.getGooseMcpExtensionOverrides();
  } finally {
    process.chdir(previousCwd);
    if (hadResourcesPath) {
      Object.defineProperty(processWithResources, 'resourcesPath', {
        value: previousResourcesPath,
        writable: true,
        configurable: true,
      });
    } else {
      Reflect.deleteProperty(processWithResources, 'resourcesPath');
    }
  }
}

test('Goose MCP extension overrides use goosed extension_override shape', () => {
  const overrides = loadGooseMcpExtensionOverrides({
    mcp_servers_json: JSON.stringify([
      {
        id: 'custom-stdio',
        name: 'Custom Stdio',
        enabled: true,
        transport: 'stdio',
        command: 'npx',
        args: ['-y', 'custom-mcp'],
        env: { CUSTOM_TOKEN: 'token-1' },
      },
      {
        id: 'custom-http',
        name: 'Custom HTTP',
        enabled: true,
        transport: 'streamable-http',
        url: 'https://mcp.example.test/mcp',
        env: { MCP_HEADER: 'header-1' },
      },
      {
        id: 'disabled-stdio',
        name: 'Disabled Stdio',
        enabled: false,
        transport: 'stdio',
        command: 'node',
        args: ['disabled.js'],
      },
    ]),
  });
  const byName = new Map(overrides.map((extension) => [extension.name, extension]));
  const beav = byName.get('beav');
  const customStdio = byName.get('custom-stdio');
  const customHttp = byName.get('custom-http');

  assert.ok(beav, 'built-in Beav MCP override should be present');
  assert.equal(beav.type, 'stdio');
  assert.equal(beav.name, 'beav');
  assert.equal(beav.description, 'Beav Client Tools MCP bridge');
  assert.equal(beav.cmd, 'node');
  assert.ok(Array.isArray(beav.args));
  assert.match(String((beav.args as string[])[0]), /beav-mcp[/\\]src[/\\]server\.js$/);
  assert.deepEqual(beav.envs, {
    BEAV_BRIDGE_URL: 'http://127.0.0.1:23456',
    BEAV_BRIDGE_PATH: '/mcp/beav',
  });
  assert.deepEqual(beav.env_keys, []);
  assert.equal(beav.timeout, 120);
  assert.equal(beav.bundled, true);
  assert.deepEqual(beav.available_tools, []);

  assert.deepEqual(customStdio, {
    type: 'stdio',
    name: 'custom-stdio',
    description: 'Custom Stdio MCP bridge',
    cmd: 'npx',
    args: ['-y', 'custom-mcp'],
    envs: { CUSTOM_TOKEN: 'token-1' },
    env_keys: [],
    timeout: 120,
    bundled: true,
    available_tools: [],
  });
  assert.deepEqual(customHttp, {
    type: 'streamable_http',
    name: 'custom-http',
    description: 'Custom HTTP MCP bridge',
    uri: 'https://mcp.example.test/mcp',
    envs: { MCP_HEADER: 'header-1' },
    env_keys: [],
    timeout: 120,
    bundled: true,
    available_tools: [],
  });
  assert.equal(byName.has('disabled-stdio'), false);
});
