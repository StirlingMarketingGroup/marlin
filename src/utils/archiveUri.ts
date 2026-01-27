export const ARCHIVE_SCHEME = 'archive';

const normalizeSlashes = (value: string) => value.replace(/\\/g, '/').replace(/\/+/g, '/');

export const normalizeArchiveInternalPath = (input?: string | null): string => {
  const raw = (input ?? '').trim();
  if (!raw) return '/';

  const replaced = normalizeSlashes(raw);
  const withLeading = replaced.startsWith('/') ? replaced : `/${replaced}`;
  const trimmed = withLeading.replace(/\/+$/g, '') || '/';

  const segments = trimmed.split('/').filter((segment) => segment.length > 0 && segment !== '.');

  if (segments.some((segment) => segment === '..')) {
    return '/';
  }

  return segments.length > 0 ? `/${segments.join('/')}` : '/';
};

export const isArchiveUri = (value?: string | null): boolean => {
  if (!value) return false;
  return value.startsWith(`${ARCHIVE_SCHEME}://`);
};

export const buildArchiveUri = (src: string, path: string = '/'): string => {
  const normalizedPath = normalizeArchiveInternalPath(path);
  const encodedSrc = encodeURIComponent(src);
  const encodedPath = encodeURIComponent(normalizedPath);
  return `${ARCHIVE_SCHEME}:///?src=${encodedSrc}&path=${encodedPath}`;
};

export const parseArchiveUri = (uri: string): { src: string; path: string } | null => {
  try {
    const url = new URL(uri);
    if (url.protocol !== `${ARCHIVE_SCHEME}:`) return null;
    const src = url.searchParams.get('src');
    if (!src) return null;
    const path = url.searchParams.get('path') ?? '/';
    return { src, path: normalizeArchiveInternalPath(path) };
  } catch {
    return null;
  }
};

export const getArchiveParentUri = (uri: string): string | null => {
  const parsed = parseArchiveUri(uri);
  if (!parsed) return null;
  const normalized = normalizeArchiveInternalPath(parsed.path);
  if (normalized === '/') {
    if (isArchiveUri(parsed.src)) {
      return parsed.src;
    }
    const schemeMatch = parsed.src.match(/^([a-zA-Z][a-zA-Z0-9+.-]*):\/\//);
    if (schemeMatch) {
      const afterScheme = parsed.src.slice(schemeMatch[0].length);
      const slashIndex = afterScheme.indexOf('/');
      if (slashIndex === -1) return parsed.src;
      const authority = afterScheme.slice(0, slashIndex);
      const pathPart = afterScheme.slice(slashIndex).replace(/\/+$/g, '') || '/';
      const lastSlash = pathPart.lastIndexOf('/');
      const parentPath = lastSlash <= 0 ? '/' : pathPart.slice(0, lastSlash);
      return `${schemeMatch[1]}://${authority}${parentPath}`;
    }
    const normalizedSrc = normalizeSlashes(parsed.src).replace(/\/+$/g, '');
    const lastSlash = normalizedSrc.lastIndexOf('/');
    if (lastSlash <= 0) return normalizedSrc || '/';
    return normalizedSrc.slice(0, lastSlash) || '/';
  }
  const trimmed = normalized.replace(/\/+$/g, '');
  const lastSlash = trimmed.lastIndexOf('/');
  const parent = lastSlash <= 0 ? '/' : trimmed.slice(0, lastSlash);
  return buildArchiveUri(parsed.src, parent);
};

const formatArchiveSource = (src: string): string => {
  if (src.startsWith('file://')) {
    try {
      const url = new URL(src);
      return decodeURIComponent(url.pathname);
    } catch {
      return src;
    }
  }
  return src;
};

export const formatArchivePathForDisplay = (uri: string): string => {
  const parsed = parseArchiveUri(uri);
  if (!parsed) return uri;
  const srcDisplay = formatArchiveSource(parsed.src);
  const internal = parsed.path === '/' ? '' : parsed.path;
  if (!internal) return srcDisplay;
  return `${srcDisplay}${internal}`;
};
