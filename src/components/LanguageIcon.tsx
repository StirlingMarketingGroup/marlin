type Size = 'small' | 'large';

interface LanguageIconProps {
  label: string;
  bg: string;
  color?: string;
  size?: Size;
  className?: string;
  title?: string;
}

export function LanguageIcon({
  label,
  bg,
  color,
  size = 'small',
  className = '',
  title,
}: LanguageIconProps) {
  const textClass = size === 'small' ? 'text-[9px]' : 'text-[12px]';
  const radius = size === 'small' ? 'rounded-[3px]' : 'rounded-[4px]';
  return (
    <div
      className={`inline-flex items-center justify-center font-semibold ${radius} ${className}`}
      style={{
        backgroundColor: bg,
        color: color || pickTextColor(bg),
        width: size === 'small' ? 20 : 48,
        height: size === 'small' ? 20 : 48,
      }}
      title={title || label}
      aria-hidden
    >
      <span
        className={`${textClass} leading-none select-none`}
        style={{ transform: 'translateY(0.5px)' }}
      >
        {label}
      </span>
    </div>
  );
}

function pickTextColor(bgHex: string): string {
  // Simple luminance-based contrast to choose white/black text
  const hex = bgHex.replace('#', '');
  const r = parseInt(hex.substring(0, 2) || '0', 16);
  const g = parseInt(hex.substring(2, 4) || '0', 16);
  const b = parseInt(hex.substring(4, 6) || '0', 16);
  const luminance = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return luminance > 0.6 ? '#111827' : '#ffffff'; // slate-900 vs white
}

export type LangMeta = { label: string; bg: string; color?: string };

// Map file extension or special filenames to a label + color
export function resolveLangIcon(name: string, ext?: string): LangMeta | undefined {
  const filename = name.toLowerCase();
  const e = (ext || '').toLowerCase();

  // Special filenames without extensions
  if (filename === 'dockerfile' || filename.startsWith('dockerfile'))
    return { label: 'DK', bg: '#0db7ed' };
  if (filename === 'makefile') return { label: 'MK', bg: '#6b7280' };
  if (filename === 'cmakelists.txt') return { label: 'CMake', bg: '#064F8C' };
  if (filename === 'rakefile') return { label: 'RB', bg: '#CC342D' };

  // Package/manifest files
  if (filename === 'package.json') return { label: 'JS', bg: '#f7df1e', color: '#000000' };
  if (filename === 'tsconfig.json') return { label: 'TS', bg: '#3178c6' };
  if (filename === 'cargo.toml') return { label: 'RS', bg: '#f74c00' };
  if (filename === 'cargo.lock') return { label: 'RS', bg: '#f74c00' };
  if (filename === 'pyproject.toml' || filename === 'requirements.txt')
    return { label: 'PY', bg: '#3776AB' };
  if (filename === 'composer.json') return { label: 'PHP', bg: '#777BB4' };
  if (filename === 'go.mod' || filename === 'go.sum') return { label: 'GO', bg: '#00ADD8' };

  // Extensions
  const map: Record<string, LangMeta> = {
    // Web
    html: { label: 'HTML', bg: '#e34f26' },
    htm: { label: 'HTML', bg: '#e34f26' },
    css: { label: 'CSS', bg: '#264de4' },
    scss: { label: 'SCSS', bg: '#c6538c' },
    sass: { label: 'SASS', bg: '#c69' },
    less: { label: 'LESS', bg: '#1d365d' },
    js: { label: 'JS', bg: '#f7df1e', color: '#000000' },
    mjs: { label: 'JS', bg: '#f7df1e', color: '#000000' },
    cjs: { label: 'JS', bg: '#f7df1e', color: '#000000' },
    jsx: { label: 'JSX', bg: '#61dafb', color: '#000000' },
    ts: { label: 'TS', bg: '#3178c6' },
    tsx: { label: 'TSX', bg: '#3178c6' },
    vue: { label: 'VUE', bg: '#41b883', color: '#0b5341' },
    svelte: { label: 'SV', bg: '#FF3E00' },
    astro: { label: 'AST', bg: '#1b1f24' },

    // Backend
    go: { label: 'GO', bg: '#00ADD8' },
    rs: { label: 'RS', bg: '#f74c00' },
    py: { label: 'PY', bg: '#3776AB' },
    rb: { label: 'RB', bg: '#CC342D' },
    php: { label: 'PHP', bg: '#777BB4' },
    java: { label: 'JAVA', bg: '#b07219' },
    kt: { label: 'KT', bg: '#7F52FF' },
    swift: { label: 'SW', bg: '#F05138' },
    cs: { label: 'CS', bg: '#239120' },
    c: { label: 'C', bg: '#00599C' },
    h: { label: 'H', bg: '#00599C' },
    cc: { label: 'C++', bg: '#00599C' },
    cpp: { label: 'C++', bg: '#00599C' },
    cxx: { label: 'C++', bg: '#00599C' },
    hpp: { label: 'H++', bg: '#00599C' },
    mm: { label: 'MM', bg: '#424242' },
    m: { label: 'M', bg: '#424242' },

    // Scripts
    sh: { label: 'SH', bg: '#3e3e3e' },
    bash: { label: 'SH', bg: '#3e3e3e' },
    zsh: { label: 'SH', bg: '#3e3e3e' },
    ps1: { label: 'PS', bg: '#012456' },
    bat: { label: 'BAT', bg: '#3e3e3e' },

    // Data/Config
    json: { label: 'JSON', bg: '#6b7280' },
    yaml: { label: 'YAML', bg: '#cb171e' },
    yml: { label: 'YAML', bg: '#cb171e' },
    toml: { label: 'TOML', bg: '#6b7280' },
    ini: { label: 'INI', bg: '#6b7280' },
    xml: { label: 'XML', bg: '#0b74aa' },
    sql: { label: 'SQL', bg: '#0064a5' },
    env: { label: 'ENV', bg: '#10b981' },

    // Others
    lua: { label: 'LUA', bg: '#000080' },
    r: { label: 'R', bg: '#276dc3' },
    dart: { label: 'DART', bg: '#0175C2' },
    ex: { label: 'EX', bg: '#4B275F' },
    exs: { label: 'EXS', bg: '#4B275F' },
    erl: { label: 'ERL', bg: '#a90533' },
    scala: { label: 'SC', bg: '#DC322F' },
    md: { label: 'MD', bg: '#0f172a' },
  };

  return map[e];
}
