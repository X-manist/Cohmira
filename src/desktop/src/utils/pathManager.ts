import {
    extractLocalAssetPathCandidate,
    isFileUrl,
    isLegacyLocalFileUrl,
    isLocalAssetSource,
    isRedboxAssetUrl,
    toRedboxAssetUrl,
} from '../../shared/localAsset';
import { convertFileSrc } from '../compat/tauri-core';

const SAFE_RENDERABLE_PROTOCOL = /^(https?:|data:|blob:|file:)/i;
const IMAGE_FILE_HINT = /\.(png|jpe?g|webp|gif|bmp|svg|avif)(?:[?#].*)?$/i;

function toFileUrl(pathValue: string): string {
    const normalized = String(pathValue || '').trim().replace(/\\/g, '/');
    if (!normalized) return '';
    if (/^[a-zA-Z]:\//.test(normalized)) {
        return `file:///${encodeURI(normalized)}`;
    }
    return `file://${encodeURI(normalized)}`;
}

function toElectronAssetUrl(value: string): string {
    const candidate = extractLocalAssetPathCandidate(value);
    if (!candidate) return '';
    try {
        if (typeof window !== 'undefined' && window.__TAURI_INTERNALS__) {
            return convertFileSrc(candidate);
        }
        if (isRedboxAssetUrl(value) || isLegacyLocalFileUrl(value)) return value;
        if (isFileUrl(value) || candidate) return toRedboxAssetUrl(candidate);
    } catch {
        return toFileUrl(candidate);
    }
    return toFileUrl(candidate);
}

export function resolveAssetUrl(value: string | null | undefined): string {
    const raw = String(value || '').trim();
    if (!raw) return '';
    if (/^(https?:|data:|blob:)/i.test(raw)) return raw;
    if (isLocalAssetSource(raw)) return toElectronAssetUrl(raw) || raw;
    if (SAFE_RENDERABLE_PROTOCOL.test(raw)) return raw;
    return raw;
}

export function hasRenderableAssetUrl(value: string | null | undefined): boolean {
    const raw = String(value || '').trim();
    if (!raw) return false;
    if (/^javascript:/i.test(raw)) return false;
    if (SAFE_RENDERABLE_PROTOCOL.test(raw)) return true;
    if (isLocalAssetSource(raw)) return true;
    return IMAGE_FILE_HINT.test(raw);
}

export function isLocalAssetUrl(value: string | null | undefined): boolean {
    return isLocalAssetSource(String(value || '').trim());
}
