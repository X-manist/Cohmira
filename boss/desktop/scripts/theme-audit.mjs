import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const css = readFileSync(resolve(root, 'src/index.css'), 'utf8');
const main = readFileSync(resolve(root, 'src/main.ts'), 'utf8');
const html = readFileSync(resolve(root, 'index.html'), 'utf8');
const themeInit = readFileSync(resolve(root, 'public/theme-init.js'), 'utf8');

function block(selector) {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = css.match(new RegExp(`${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`));
  assert.ok(match, `missing ${selector} block`);
  return match[1];
}

function hexToken(source, name) {
  const match = source.match(new RegExp(`--${name}:\\s*(#[0-9a-f]{6});`, 'i'));
  assert.ok(match, `missing --${name}`);
  const value = match[1].slice(1);
  return [0, 2, 4].map((index) => Number.parseInt(value.slice(index, index + 2), 16));
}

function luminance(rgb) {
  const channels = rgb.map((channel) => {
    const normalized = channel / 255;
    return normalized <= 0.04045
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  });
  return (0.2126 * channels[0]) + (0.7152 * channels[1]) + (0.0722 * channels[2]);
}

function contrast(left, right) {
  const values = [luminance(left), luminance(right)].sort((a, b) => b - a);
  return (values[0] + 0.05) / (values[1] + 0.05);
}

const light = block(':root');
const dark = block("[data-theme='dark']");
const requiredTokens = [
  'ink-900', 'ink-500', 'ink-75', 'ink-50', 'surface', 'on-solid', 'on-accent',
  'sidebar-bg', 'sidebar-muted', 'strong-surface', 'blue', 'blue-soft', 'green', 'green-soft',
  'orange', 'orange-soft', 'red', 'red-soft',
];
for (const token of requiredTokens) {
  hexToken(light, token);
  hexToken(dark, token);
}

for (const [name, theme] of [['light', light], ['dark', dark]]) {
  for (const background of ['ink-75', 'ink-50', 'surface']) {
    assert.ok(
      contrast(hexToken(theme, 'ink-900'), hexToken(theme, background)) >= 4.5,
      `${name} primary text fails WCAG AA on ${background}`,
    );
    assert.ok(
      contrast(hexToken(theme, 'ink-500'), hexToken(theme, background)) >= 4.5,
      `${name} secondary text fails WCAG AA on ${background}`,
    );
  }

  for (const accent of ['blue', 'orange']) {
    assert.ok(
      contrast(hexToken(theme, 'on-accent'), hexToken(theme, accent)) >= 4.5,
      `${name} ${accent} button label fails WCAG AA`,
    );
  }

  for (const status of ['blue', 'green', 'orange', 'red']) {
    assert.ok(
      contrast(hexToken(theme, status), hexToken(theme, 'surface')) >= 4.5,
      `${name} ${status} status text fails WCAG AA`,
    );
  }

  assert.ok(
    contrast(hexToken(theme, 'sidebar-muted'), hexToken(theme, 'sidebar-bg')) >= 4.5,
    `${name} sidebar secondary text fails WCAG AA`,
  );
}

assert.doesNotMatch(css, /var\(--white\)/, 'legacy white token can create light islands');
assert.match(main, /setAttribute\('data-theme', mode\)/);
assert.match(main, /classList\.toggle\('dark', mode === 'dark'\)/);
assert.match(main, /localStorage\.setItem\(THEME_STORAGE_KEY, state\.themeMode\)/);
assert.equal((main.match(/data-toggle-theme/g) || []).length >= 3, true, 'theme control must render and bind on login/workspace');
assert.match(html, /<script src="\/theme-init\.js"><\/script>/, 'theme must initialize before the app entrypoint');
assert.match(themeInit, /prefers-color-scheme: dark/);
assert.match(themeInit, /if \(!mode\)/, 'OS preference must remain available when storage access fails');
assert.match(themeInit, /setAttribute\('data-theme', mode\)/);

const cssWithoutTokens = css.replace(/:root\s*\{[\s\S]*?\n\}/, '');
assert.doesNotMatch(
  cssWithoutTokens,
  /background:\s*(?:#f[0-9a-f]{5}|rgba\(255,\s*255,\s*255,\s*0\.9[0-9]*\))/i,
  'fixed light application surface bypasses theme tokens',
);

console.log('[theme-audit] boss dark/light theme semantics OK');
