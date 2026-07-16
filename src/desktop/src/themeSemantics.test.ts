import { readdirSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const readSource = (relativePath: string) => readFileSync(resolve(process.cwd(), 'src', relativePath), 'utf8');
const readProjectFile = (relativePath: string) => readFileSync(resolve(process.cwd(), relativePath), 'utf8');

function businessSources(relativeDirectory = ''): string[] {
  const directory = resolve(process.cwd(), 'src', relativeDirectory);
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const relativePath = relativeDirectory ? `${relativeDirectory}/${entry.name}` : entry.name;
    if (entry.isDirectory()) {
      return entry.name === 'vendor' ? [] : businessSources(relativePath);
    }
    if (!entry.isFile() || !/\.[jt]sx?$/.test(entry.name) || entry.name.includes('.test.')) return [];
    return [readSource(relativePath)];
  });
}

function themeBlock(css: string, selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const match = css.match(new RegExp(`${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`));
  if (!match) throw new Error(`Missing theme block: ${selector}`);
  return match[1];
}

function rgbToken(block: string, name: string): [number, number, number] {
  const match = block.match(new RegExp(`--${name}:\\s*(\\d+)\\s+(\\d+)\\s+(\\d+);`));
  if (!match) throw new Error(`Missing RGB token: ${name}`);
  return [Number(match[1]), Number(match[2]), Number(match[3])];
}

function relativeLuminance([red, green, blue]: [number, number, number]): number {
  const [r, g, b] = [red, green, blue].map((channel) => {
    const normalized = channel / 255;
    return normalized <= 0.04045
      ? normalized / 12.92
      : ((normalized + 0.055) / 1.055) ** 2.4;
  });
  return (0.2126 * r) + (0.7152 * g) + (0.0722 * b);
}

function contrastRatio(left: [number, number, number], right: [number, number, number]): number {
  const lighter = Math.max(relativeLuminance(left), relativeLuminance(right));
  const darker = Math.min(relativeLuminance(left), relativeLuminance(right));
  return (lighter + 0.05) / (darker + 0.05);
}

describe('semantic theme coverage', () => {
  it('applies the persisted or OS theme before the employee app entrypoint', () => {
    const html = readProjectFile('index.html');
    const themeInit = readProjectFile('public/theme-init.js');

    expect(html).toContain('<script src="/theme-init.js"></script>');
    expect(html.indexOf('/theme-init.js')).toBeLessThan(html.indexOf('/src/main.tsx'));
    expect(themeInit).toContain("localStorage.getItem('redbox:theme-mode:v1')");
    expect(themeInit).toContain('prefers-color-scheme: dark');
    expect(themeInit).toContain("setAttribute('data-theme', mode)");
    expect(themeInit).toContain("classList.toggle('dark', mode === 'dark')");
  });

  it('keeps Workboard and RedClaw onboarding free of fixed light palettes', () => {
    const workboard = readSource('pages/Workboard.tsx');
    const onboarding = readSource('pages/redclaw/RedClawOnboardingFlow.tsx');
    const automationDrawer = readSource('pages/redclaw/RedClawAutomationDrawer.tsx');
    const css = readSource('index.css');

    expect(workboard).not.toMatch(/#[0-9a-f]{3,8}|rgba?\(/i);
    expect(workboard).not.toContain('bg-white');
    expect(onboarding).not.toMatch(/#[0-9a-f]{3,8}|rgba\(|(?:text|bg|border)-stone-/i);
    expect(onboarding).not.toContain('bg-white');
    expect(automationDrawer).not.toMatch(/#[0-9a-f]{3,8}|rgba\(|(?:text|bg|border)-(?:stone|slate|gray|zinc|neutral)-/i);
    expect(automationDrawer).not.toContain('bg-white');
    expect(automationDrawer).toContain('grid-cols-1 gap-2 sm:grid-cols-3');
    expect(automationDrawer.match(/grid-cols-1 gap-3 sm:grid-cols-2/g)?.length).toBe(2);
    expect(css).not.toMatch(/\.workboard-shell[\s\S]{0,180}color-scheme:\s*light/);
    expect(workboard).toContain('xl:h-full xl:min-h-0');
    expect(workboard).toContain('max-h-[440px]');
    expect(workboard).toContain('xl:max-h-none');
  });

  it('maps every legacy light status palette to a dark semantic surface', () => {
    const css = readSource('index.css');
    const manuscripts = readSource('pages/Manuscripts.tsx');
    const main = readSource('main.tsx');
    const layout = readSource('components/Layout.tsx');
    const paletteFamilies = [
      'red', 'rose', 'pink', 'amber', 'orange', 'yellow', 'green', 'emerald',
      'teal', 'blue', 'sky', 'cyan', 'indigo', 'violet', 'purple',
    ];

    for (const family of paletteFamilies) {
      expect(css, `missing dark mapping for bg-${family}-50`).toContain(
        `[data-theme='dark'] .bg-${family}-50`,
      );
    }

    expect(manuscripts).not.toMatch(/(?:from|via|to)-\[#[0-9a-f]{3,8}\]|to-white/i);
    expect(manuscripts).not.toMatch(/(?:bg-white\/(?:70|80|85|90)|border-white\/(?:30|60))/);
    expect(manuscripts.match(/theme-content-light/g)?.length).toBeGreaterThanOrEqual(3);
    expect(main).toContain("classList.toggle('dark', mode === 'dark')");
    expect(layout).toContain("classList.toggle('dark', effectiveTheme === 'dark')");
    expect(css).toContain('.bg-text-primary.text-white');
    expect(css).toContain('.bg-amber-500.text-white');
    expect(css).toContain('.bg-emerald-500.text-white');
    expect(css).toContain('.text-brand-red');
  });

  it('covers every light status background utility used by business components', () => {
    const css = readSource('index.css');
    const utilityPattern = /\b(?:hover:)?bg-(?:red|rose|pink|amber|orange|yellow|green|emerald|teal|blue|sky|cyan|indigo|violet|purple)-(?:50|100)(?:\/\d+)?/g;
    const utilities = new Set(
      businessSources().flatMap((source) => Array.from(source.matchAll(utilityPattern), ([utility]) => utility)),
    );

    for (const utility of utilities) {
      const selector = utility.replaceAll(':', '\\:').replaceAll('/', '\\/');
      const state = utility.startsWith('hover:') ? ':hover' : '';
      expect(css, `missing dark semantic mapping for ${utility}`).toContain(
        `[data-theme='dark'] .${selector}${state}`,
      );
    }
  });

  it.each([
    [':root', 'light'],
    ["[data-theme='dark']", 'dark'],
  ])('%s text tokens meet WCAG AA on all app surfaces', (selector) => {
    const css = readSource('index.css');
    const block = themeBlock(css, selector);
    const foregrounds = ['color-text-primary', 'color-text-secondary', 'color-text-tertiary'];
    const backgrounds = ['color-background', 'color-surface-primary', 'color-surface-secondary', 'color-surface-elevated'];

    for (const foreground of foregrounds) {
      for (const background of backgrounds) {
        expect(
          contrastRatio(rgbToken(block, foreground), rgbToken(block, background)),
          `${selector}: ${foreground} on ${background}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }

    expect(
      contrastRatio(rgbToken(block, 'color-accent-primary'), rgbToken(block, 'color-on-accent')),
      `${selector}: on-accent label`,
    ).toBeGreaterThanOrEqual(4.5);
    expect(
      contrastRatio(rgbToken(block, 'color-brand-red'), rgbToken(block, 'color-on-brand')),
      `${selector}: on-brand label`,
    ).toBeGreaterThanOrEqual(4.5);

    const semanticForegrounds = [
      'color-accent-primary',
      'color-status-success',
      'color-status-warning',
      'color-status-error',
      'color-brand-red-text',
    ];
    for (const foreground of semanticForegrounds) {
      for (const background of ['color-background', 'color-surface-primary', 'color-surface-secondary']) {
        expect(
          contrastRatio(rgbToken(block, foreground), rgbToken(block, background)),
          `${selector}: ${foreground} on ${background}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    }

    expect(
      contrastRatio(rgbToken(block, 'color-text-primary'), rgbToken(block, 'color-on-accent')),
      `${selector}: text-primary solid label`,
    ).toBeGreaterThanOrEqual(4.5);
  });
});
