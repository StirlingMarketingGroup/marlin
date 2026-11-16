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
  // The backend also encodes !'()*. A URI built here must equal the one it lists for the same entry.
  const encode = (value: string) =>
    encodeURIComponent(value).replace(
      /[!'()*]/g,
      (char) => `%${char.charCodeAt(0).toString(16).toUpperCase()}`
    );
  const encodedSrc = encode(src);
  const encodedPath = encode(normalizedPath);
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
