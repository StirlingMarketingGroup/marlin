import { useEffect, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';

const GITHUB_RELEASES_API =
  'https://api.github.com/repos/StirlingMarketingGroup/marlin/releases/latest';
const RELEASES_PAGE = 'https://github.com/StirlingMarketingGroup/marlin/releases';
const CACHE_KEY = 'marlin:update-info';
const CACHE_TTL_MS = 6 * 60 * 60 * 1000; // 6 hours

type GithubRelease = {
  tag_name: string;
  html_url?: string;
  draft?: boolean;
  prerelease?: boolean;
};

type UpdateCache = {
  latestVersion: string;
  releaseUrl: string;
  checkedAt: number;
};

type UpdateState = {
  checking: boolean;
  updateAvailable: boolean;
  currentVersion: string | null;
  latestVersion: string | null;
  releaseUrl: string;
  error: string | null;
};

const initialState: UpdateState = {
  checking: true,
  updateAvailable: false,
  currentVersion: null,
  latestVersion: null,
  releaseUrl: RELEASES_PAGE,
  error: null,
};

const safeParse = (version: string | null | undefined) => (version || '').trim().replace(/^v/i, '');

const isUpdateCache = (value: unknown): value is UpdateCache => {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.latestVersion === 'string' &&
    candidate.latestVersion.trim() !== '' &&
    typeof candidate.releaseUrl === 'string' &&
    candidate.releaseUrl.trim() !== '' &&
    typeof candidate.checkedAt === 'number' &&
    Number.isFinite(candidate.checkedAt)
  );
};

const isGithubRelease = (value: unknown): value is GithubRelease => {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Record<string, unknown>;
  return (
    typeof candidate.tag_name === 'string' &&
    candidate.tag_name.trim() !== '' &&
    (candidate.html_url === undefined || typeof candidate.html_url === 'string') &&
    (candidate.draft === undefined || typeof candidate.draft === 'boolean') &&
    (candidate.prerelease === undefined || typeof candidate.prerelease === 'boolean')
  );
};

const extractNumericSegments = (version: string) => {
  const core = version.split('-')[0];
  return core
    .split('.')
    .map((segment) => Number.parseInt(segment, 10))
    .filter((segment) => Number.isFinite(segment));
};

const isVersionNewer = (current: string, latest: string) => {
  const currentSegments = extractNumericSegments(safeParse(current));
  const latestSegments = extractNumericSegments(safeParse(latest));
  const length = Math.max(currentSegments.length, latestSegments.length);

  for (let i = 0; i < length; i += 1) {
    const currentValue = currentSegments[i] ?? 0;
    const latestValue = latestSegments[i] ?? 0;
    if (latestValue > currentValue) return true;
    if (latestValue < currentValue) return false;
  }

  return false;
};

const loadCache = (): UpdateCache | null => {
  if (typeof window === 'undefined') return null;
  try {
    const raw = window.localStorage.getItem(CACHE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw);
    if (!isUpdateCache(parsed)) {
      window.localStorage.removeItem(CACHE_KEY);
      return null;
    }
    const latestVersion = safeParse(parsed.latestVersion);
    if (!latestVersion) {
      window.localStorage.removeItem(CACHE_KEY);
      return null;
    }
    const releaseUrl = parsed.releaseUrl.trim();
    if (!releaseUrl) {
      window.localStorage.removeItem(CACHE_KEY);
      return null;
    }
    if (Date.now() - parsed.checkedAt > CACHE_TTL_MS) {
      window.localStorage.removeItem(CACHE_KEY);
      return null;
    }
    return {
      latestVersion,
      releaseUrl,
      checkedAt: parsed.checkedAt,
    };
  } catch (error) {
    console.warn('Failed to read cached update info:', error);
    if (typeof window !== 'undefined') {
      window.localStorage.removeItem(CACHE_KEY);
    }
    return null;
  }
};

const saveCache = (payload: UpdateCache) => {
  if (typeof window === 'undefined') return;
  try {
    window.localStorage.setItem(CACHE_KEY, JSON.stringify(payload));
  } catch (error) {
    console.warn('Failed to cache update info:', error);
  }
};

const resolveCurrentVersion = async (): Promise<string> => {
  try {
    return await getVersion();
  } catch (error) {
    console.warn('Failed to retrieve app version from Tauri runtime:', error);
  }

  if (typeof import.meta !== 'undefined') {
    const env = import.meta.env as Record<string, string | undefined>;
    if (env?.VITE_APP_VERSION) return env.VITE_APP_VERSION;
    if (env?.TAURI_APP_VERSION) return env.TAURI_APP_VERSION;
    if (env?.npm_package_version) return env.npm_package_version;
  }

  return '0.0.0';
};

const fetchLatestRelease = async (): Promise<UpdateCache> => {
  const response = await fetch(GITHUB_RELEASES_API, {
    headers: { Accept: 'application/vnd.github+json' },
  });

  if (!response.ok) {
    throw new Error(`GitHub responded with ${response.status}`);
  }

  const data = await response.json();
  if (!isGithubRelease(data)) {
    throw new Error('Unexpected response format from GitHub releases API');
  }
  const latestVersion = safeParse(data.tag_name);
  const releaseUrl = data.html_url?.trim() || RELEASES_PAGE;

  if (!latestVersion) {
    throw new Error('Latest release is missing a tag name');
  }

  const payload: UpdateCache = {
    latestVersion,
    releaseUrl,
    checkedAt: Date.now(),
  };

  saveCache(payload);
  return payload;
};

export function useUpdateCheck() {
  const [state, setState] = useState<UpdateState>(initialState);

  useEffect(() => {
    let disposed = false;

    const checkUpdates = async () => {
      setState((prev) => ({ ...prev, checking: true, error: null }));

      try {
        const currentVersion = await resolveCurrentVersion();
        if (disposed) return;

        const cached = loadCache();
        const base = cached ?? (await fetchLatestRelease());
        if (disposed) return;

        // If cache was used but stale, refresh in background once
        if (cached) {
          fetchLatestRelease()
            .then((fresh) => {
              if (disposed) return;
              setState((prev) => ({
                ...prev,
                latestVersion: fresh.latestVersion,
                releaseUrl: fresh.releaseUrl,
                updateAvailable: isVersionNewer(currentVersion, fresh.latestVersion),
              }));
            })
            .catch((error) => {
              console.warn('Background update check failed:', error);
            });
        }

        setState({
          checking: false,
          currentVersion,
          latestVersion: base.latestVersion,
          releaseUrl: base.releaseUrl,
          updateAvailable: isVersionNewer(currentVersion, base.latestVersion),
          error: null,
        });
      } catch (error) {
        if (disposed) return;
        console.warn('Failed to check for updates:', error);
        setState((prev) => ({
          ...prev,
          checking: false,
          updateAvailable: false,
          error: error instanceof Error ? error.message : String(error),
        }));
      }
    };

    checkUpdates();

    return () => {
      disposed = true;
    };
  }, []);

  return state;
}
