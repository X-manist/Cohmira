function extensionFromUrl(source: string): string {
  const clean = source.split(/[?#]/, 1)[0] || '';
  const match = clean.toLowerCase().match(/\.([a-z0-9]+)$/);
  return match?.[1] || '';
}

export function isGifUrl(source: string | null | undefined): boolean {
  return extensionFromUrl(String(source || '')) === 'gif';
}

export function isWebpUrl(source: string | null | undefined): boolean {
  return extensionFromUrl(String(source || '')) === 'webp';
}
