import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { describe, it } from 'node:test';

import { frontendBuildInvocation, npmInvocation } from './tauri-build-lib.mjs';

describe('Tauri build command contract', () => {
  it('launches the npm Windows shim through cmd.exe', () => {
    assert.deepEqual(
      frontendBuildInvocation({ platform: 'win32', comSpec: 'C:\\Windows\\System32\\cmd.exe' }),
      {
        command: 'C:\\Windows\\System32\\cmd.exe',
        args: ['/d', '/s', '/c', 'npm.cmd', 'run', 'build'],
      },
    );
  });

  it('falls back to cmd.exe when ComSpec is unavailable', () => {
    assert.equal(
      frontendBuildInvocation({ platform: 'win32', comSpec: '' }).command,
      'cmd.exe',
    );
  });

  it('launches npm directly on non-Windows platforms', () => {
    assert.deepEqual(frontendBuildInvocation({ platform: 'darwin' }), {
      command: 'npm',
      args: ['run', 'build'],
    });
  });

  it('executes the npm Windows shim through the selected command', {
    skip: process.platform !== 'win32',
  }, () => {
    const invocation = npmInvocation(['--version']);
    const result = spawnSync(invocation.command, invocation.args, {
      encoding: 'utf8',
      env: process.env,
    });

    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout.trim(), /^\d+\.\d+\.\d+/);
  });
});
