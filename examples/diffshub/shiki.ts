// Shiki is not in this app: diffs highlight with `highlights`. Only `normalizeTheme` stays, because
// `@pierre/theming` loads every color theme through it. It follows `normalizeTheme` of `@shikijs/primitive`.
const stripped = () => {
  throw new Error('Shiki is not in this app. Use preferredHighlighter: "highlights".')
}

type Setting = { name?: string; scope?: unknown; settings?: { foreground?: string; background?: string } }
type Theme = {
  type?: string
  fg?: string
  bg?: string
  colors?: Record<string, string>
  colorReplacements?: Record<string, string>
  settings?: Setting[]
  tokenColors?: Setting[]
  [key: string]: unknown
}

const RESOLVED = '__shiki_resolved'

export function normalizeTheme(raw: Theme): Theme {
  if (raw?.[RESOLVED]) return raw
  const theme: Theme = { ...raw }
  if (theme.tokenColors && !theme.settings) {
    theme.settings = theme.tokenColors
    delete theme.tokenColors
  }
  theme.type ||= 'dark'
  const replaced: Record<string, string> = (theme.colorReplacements = { ...theme.colorReplacements })
  const settings = (theme.settings ||= [])
  if (!theme.bg || !theme.fg) {
    const global = settings.find((setting) => !setting.name && !setting.scope)
    const light = theme.type === 'light'
    theme.fg = global?.settings?.foreground || theme.fg || theme.colors?.['editor.foreground'] || (light ? '#333333' : '#bbbbbb')
    theme.bg = global?.settings?.background || theme.bg || theme.colors?.['editor.background'] || (light ? '#fffffe' : '#1e1e1e')
  }
  if (!(settings[0]?.settings && !settings[0].scope)) {
    settings.unshift({ settings: { foreground: theme.fg, background: theme.bg } })
  }
  // Colors that are not hex, like CSS variables, get placeholder hex colors.
  const replacements = new Map<string, string>()
  let count = 0
  const replace = (value: string): string => {
    const known = replacements.get(value)
    if (known) return known
    const hex = `#${(++count).toString(16).padStart(8, '0')}`
    if (replaced[`#${hex}`]) return replace(value)
    replacements.set(value, hex)
    replaced[hex] = value
    return hex
  }
  theme.settings = settings.map((setting) => {
    const { foreground, background } = setting.settings ?? {}
    const fg = foreground && !foreground.startsWith('#')
    const bg = background && !background.startsWith('#')
    if (!fg && !bg) return setting
    return {
      ...setting,
      settings: {
        ...setting.settings,
        ...(fg && { foreground: replace(foreground) }),
        ...(bg && { background: replace(background) }),
      },
    }
  })
  for (const [key, value] of Object.entries(theme.colors || {})) {
    if ((key === 'editor.foreground' || key === 'editor.background' || key.startsWith('terminal.ansi')) && !value?.startsWith('#')) {
      theme.colors![key] = replace(value)
    }
  }
  Object.defineProperty(theme, RESOLVED, { enumerable: false, writable: false, value: true })
  return theme
}

export const createHighlighterCore = stripped
export const createCssVariablesTheme = stripped
export const createOnigurumaEngine = stripped
export const createJavaScriptRegexEngine = stripped
export const bundledLanguages = {}
export const EncodedTokenMetadata = {}
export const INITIAL = null
export default {}
