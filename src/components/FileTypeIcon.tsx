import { Icon, addCollection } from '@iconify/react';
import type { IconifyJSON } from '@iconify/types';
import vscodeIcons from '@iconify-json/vscode-icons/icons.json';

// Load the VSCode icon collection once
let loaded = false;
if (!loaded) {
  addCollection(vscodeIcons as IconifyJSON);
  loaded = true;
}

export type FileIconSize = 'small' | 'large';

export function resolveVSCodeIcon(name: string, ext?: string): string | undefined {
  const filename = name.toLowerCase();
  const e = (ext || '').toLowerCase();

  // Special filenames
  if (filename === 'dockerfile' || filename.startsWith('dockerfile'))
    return 'vscode-icons:file-type-docker';
  if (filename === 'makefile') return 'vscode-icons:file-type-makefile';
  if (filename === 'cmakelists.txt') return 'vscode-icons:file-type-cmake';
  if (filename === 'package.json') return 'vscode-icons:file-type-npm';
  if (filename === 'tsconfig.json') return 'vscode-icons:file-type-tsconfig';
  if (filename === 'cargo.toml' || filename === 'cargo.lock') return 'vscode-icons:file-type-rust';
  if (filename === 'go.mod' || filename === 'go.sum') return 'vscode-icons:file-type-go';
  if (filename === '.env' || filename.startsWith('.env.')) return 'vscode-icons:file-type-dotenv';

  const map: Record<string, string> = {
    // Archives (use the standard VSCode zip icon for all)
    zip: 'vscode-icons:file-type-zip',
    rar: 'vscode-icons:file-type-zip',
    '7z': 'vscode-icons:file-type-zip',
    '7zip': 'vscode-icons:file-type-zip',
    tar: 'vscode-icons:file-type-zip',
    gz: 'vscode-icons:file-type-zip',
    tgz: 'vscode-icons:file-type-zip',
    bz2: 'vscode-icons:file-type-zip',
    tbz2: 'vscode-icons:file-type-zip',
    xz: 'vscode-icons:file-type-zip',
    txz: 'vscode-icons:file-type-zip',
    zst: 'vscode-icons:file-type-zip',
    lz: 'vscode-icons:file-type-zip',
    lzma: 'vscode-icons:file-type-zip',

    // Web
    html: 'vscode-icons:file-type-html',
    htm: 'vscode-icons:file-type-html',
    css: 'vscode-icons:file-type-css',
    scss: 'vscode-icons:file-type-scss',
    sass: 'vscode-icons:file-type-sass',
    less: 'vscode-icons:file-type-less',
    js: 'vscode-icons:file-type-js',
    mjs: 'vscode-icons:file-type-js',
    cjs: 'vscode-icons:file-type-js',
    jsx: 'vscode-icons:file-type-reactjs',
    ts: 'vscode-icons:file-type-typescript',
    tsx: 'vscode-icons:file-type-reactts',
    vue: 'vscode-icons:file-type-vue',
    svelte: 'vscode-icons:file-type-svelte',
    astro: 'vscode-icons:file-type-astro',

    // Backend / languages
    go: 'vscode-icons:file-type-go',
    rs: 'vscode-icons:file-type-rust',
    py: 'vscode-icons:file-type-python',
    rb: 'vscode-icons:file-type-ruby',
    php: 'vscode-icons:file-type-php',
    java: 'vscode-icons:file-type-java',
    kt: 'vscode-icons:file-type-kotlin',
    swift: 'vscode-icons:file-type-swift',
    cs: 'vscode-icons:file-type-csharp',
    c: 'vscode-icons:file-type-c',
    h: 'vscode-icons:file-type-c',
    cc: 'vscode-icons:file-type-cpp',
    cpp: 'vscode-icons:file-type-cpp',
    cxx: 'vscode-icons:file-type-cpp',
    hpp: 'vscode-icons:file-type-cpp',
    m: 'vscode-icons:file-type-objectivec',
    mm: 'vscode-icons:file-type-objectivecpp',

    // Scripts
    sh: 'vscode-icons:file-type-shell',
    bash: 'vscode-icons:file-type-shell',
    zsh: 'vscode-icons:file-type-shell',
    ps1: 'vscode-icons:file-type-powershell',
    bat: 'vscode-icons:file-type-bat',

    // Data / config
    json: 'vscode-icons:file-type-json',
    yaml: 'vscode-icons:file-type-yaml',
    yml: 'vscode-icons:file-type-yaml',
    toml: 'vscode-icons:file-type-toml',
    ini: 'vscode-icons:file-type-settings',
    xml: 'vscode-icons:file-type-xml',
    sql: 'vscode-icons:file-type-sql',
    env: 'vscode-icons:file-type-dotenv',
    md: 'vscode-icons:file-type-markdown',
    lua: 'vscode-icons:file-type-lua',
    dart: 'vscode-icons:file-type-dart',
    scala: 'vscode-icons:file-type-scala',
    r: 'vscode-icons:file-type-r',
    groovy: 'vscode-icons:file-type-groovy',
    gradle: 'vscode-icons:file-type-gradle',
  };

  return map[e];
}

export function FileTypeIcon({
  name,
  ext,
  size = 'small',
  className,
  pixelSize,
}: {
  name: string;
  ext?: string;
  size?: FileIconSize;
  className?: string;
  pixelSize?: number;
}) {
  const iconName = resolveVSCodeIcon(name, ext);
  if (!iconName) return null;
  const px = pixelSize ?? (size === 'small' ? 20 : 48);
  // Use inline SVG for icons to preserve exact alignment and crispness
  return <Icon icon={iconName} width={px} height={px} className={className} />;
}
