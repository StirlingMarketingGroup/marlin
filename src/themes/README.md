# Themes

Marlin includes a comprehensive theming system with built-in support for accessibility.

## Built-in Themes

### Dark Themes

- **Default Dark** - The default dark theme
- **GitHub Dark** - GitHub's dark color scheme
- **GitHub Dark Colorblind** - Accessible variant optimized for color vision deficiency
- **Monokai Pro** - Classic Monokai colors
- **Monokai Pro Octagon** - Monokai Pro variant
- **Gruvbox** - Retro groove color scheme
- **Solarized Dark** - Ethan Schoonover's Solarized dark
- **Dracula** - Popular dark theme
- **Catppuccin Mocha** - Soothing pastel theme (dark)
- **Nord** - Arctic, north-bluish color palette
- **Ubuntu** - Ubuntu terminal colors
- **Synthwave** - Retro 80s neon aesthetic
- **Windows Hotdog** - Classic Windows hotdog stand theme
- **Windows XP** - Windows XP Luna theme nostalgia

### Light Themes

- **Default Light** - The default light theme
- **GitHub Light** - GitHub's light color scheme
- **GitHub Light Colorblind** - Accessible variant optimized for color vision deficiency
- **Solarized Light** - Ethan Schoonover's Solarized light
- **Catppuccin Latte** - Soothing pastel theme (light)
- **Windows Hotdog Light** - Light variant of the hotdog stand theme

## Colorblind-Friendly Themes

The GitHub Colorblind themes are specifically designed for users with color vision deficiency (CVD). These themes:

- Use blue instead of green for success/positive states, avoiding red-green confusion
- Maintain sufficient contrast ratios for all color combinations
- Follow GitHub's accessibility guidelines for CVD users

## Custom Themes

### Importing Themes

You can import custom themes through Preferences → Themes → Import Theme. Supported formats:

- **JSON** - Native Marlin theme format
- **iTerm2** - `.itermcolors` files from iTerm2

### JSON Theme Format

Create a JSON file with the following structure:

```json
{
  "id": "my-custom-theme",
  "name": "My Custom Theme",
  "author": "Your Name",
  "colorScheme": "dark",
  "colors": {
    "appDark": "#1a1b26",
    "appDarker": "#16161e",
    "appGray": "#24283b",
    "appLight": "#414868",
    "text": "#c0caf5",
    "muted": "#565f89",
    "border": "#3b4261",
    "accent": "#7aa2f7",
    "green": "#9ece6a",
    "red": "#f7768e",
    "yellow": "#e0af68"
  }
}
```

### Color Properties

| Property    | Description                             |
| ----------- | --------------------------------------- |
| `appDark`   | Main background color                   |
| `appDarker` | Darker variant for depth                |
| `appGray`   | Secondary background                    |
| `appLight`  | Lighter background for hover states     |
| `text`      | Primary text color                      |
| `muted`     | Secondary/dimmed text                   |
| `border`    | Border color                            |
| `accent`    | Primary accent color (selection, focus) |
| `green`     | Success/positive color                  |
| `red`       | Error/negative color                    |
| `yellow`    | Warning color                           |

### Theme Directory

You can also place theme files in the app config directory:

- **macOS**: `~/Library/Application Support/com.marlin.filebrowser/themes/`
- **Windows**: `%APPDATA%\com.marlin.filebrowser\themes\`
- **Linux**: `~/.config/com.marlin.filebrowser/themes/`

Themes in this directory are automatically loaded on startup.

## Creating Accessible Themes

When creating custom themes, consider these accessibility guidelines:

1. **Contrast ratios**: Ensure text colors have at least 4.5:1 contrast against backgrounds
2. **Color independence**: Don't rely solely on color to convey information
3. **Colorblind considerations**: Avoid red-green combinations for status indicators
4. **Test with simulators**: Use colorblind simulation tools to verify your theme
