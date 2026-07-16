import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { validateWindowsRuntime } from './windows-runtime-lib.mjs';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const runtimeRoot = path.resolve(scriptDir, '..', 'src-tauri', 'runtime');
const result = await validateWindowsRuntime(runtimeRoot);

if (!result.ok) {
  console.error(`Windows offline runtime is incomplete:\n- ${result.missing.join('\n- ')}`);
  process.exitCode = 1;
} else {
  console.log(`Windows offline runtime is complete: ${runtimeRoot}`);
}
