import '@ui-sdk/web/button'
import '@/components/ui/display'
import { notify } from '@/components/ui/toaster'
import { haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

type Language = 'ts' | 'tsx' | 'js' | 'css' | 'sh' | 'text'

const ts =
  /(\/\/.*$|\/\*[\s\S]*?\*\/)|("(?:\\.|[^"])*"|'(?:\\.|[^'])*'|`(?:\\.|[^`])*`)|\b(import|from|export|const|let|var|function|return|if|else|for|while|await|async|new|type|interface|extends|class|default|as|of|in|true|false|null|undefined)\b|\b(\d+(?:\.\d+)?)\b/gm
const patterns: Record<Exclude<Language, 'text'>, RegExp> = {
  ts,
  tsx: ts,
  js: /(\/\/.*$|\/\*[\s\S]*?\*\/)|("(?:\\.|[^"])*"|'(?:\\.|[^'])*'|`(?:\\.|[^`])*`)|\b(import|from|export|const|let|var|function|return|if|else|for|while|await|async|new|class|default|of|in|true|false|null|undefined)\b|\b(\d+(?:\.\d+)?)\b/gm,
  css: /(\/\*[\s\S]*?\*\/)|("(?:\\.|[^"])*"|'(?:\\.|[^'])*')|(@[\w-]+|--[\w-]+)|\b(\d+(?:\.\d+)?(?:px|em|rem|%|deg|ms|s)?)\b/gm,
  sh: /(#.*$)|("(?:\\.|[^"])*"|'(?:\\.|[^'])*')|^\s*(\$|pnpm|npm|npx|git|cd|node)\b|(\B--?[\w-]+)/gm,
}
const kinds = ['comment', 'string', 'keyword', 'number']
const names: Record<Language, string> = { ts: 'TypeScript', tsx: 'TypeScript', js: 'JavaScript', css: 'CSS', sh: 'Shell', text: '' }

const highlight = (line: string, language: Language) => {
  if (language === 'text') return escape(line)
  let html = ''
  let last = 0
  for (const match of line.matchAll(patterns[language])) {
    html += `${escape(line.slice(last, match.index))}<span data-token="${kinds[match.slice(1).findIndex((group) => group !== undefined)]}">${escape(match[0])}</span>`
    last = match.index + match[0].length
  }
  return html + escape(line.slice(last))
}

class CodeBlock extends Kit {
  static observedAttributes = ['code', 'language', 'typing', 'label']
  #errors?: RegExp
  language: Language = 'text'
  lines: string[] = []
  line = 0
  chars = 0
  timer = 0
  copied = 0

  get errors() {
    return this.#errors
  }

  set errors(errors: RegExp | undefined) {
    this.#errors = errors
    if (this.rendered) this.update()
  }

  connectedCallback() {
    super.connectedCallback()
    this.type()
  }

  disconnectedCallback() {
    clearTimeout(this.timer)
  }

  render() {
    this.dataset.kit = 'code-block'
    this.update()
  }

  update() {
    const code = this.getAttribute('code') ?? ''
    const language = (this.language = (this.getAttribute('language') ?? 'text') as Language)
    const typing = this.hasAttribute('typing')
    const label = this.getAttribute('label') ?? names[language]
    const lines = (this.lines = code.replace(/\n$/, '').split('\n'))
    this.line = typing && !matchMedia('(prefers-reduced-motion: reduce)').matches ? 0 : lines.length
    this.chars = 0
    const number = (i: number) => `<span data-kit="code-number" aria-hidden="true">${String(i + 1).padStart(2, '0')}</span>`
    this.innerHTML = `${label ? `<span data-kit="code-label">${escape(label)}</span>` : ''}<div data-kit="code-body"><spec-display aspect="auto"><div data-kit="code" data-language="${language}"><ol${typing ? ' aria-live="polite"' : ''}>${lines
      .map((line, i) => {
        const pending = i > this.line ? ' data-pending aria-hidden="true"' : ''
        const current = i === this.line
        return `<li${this.#errors?.test(line) ? ' data-error' : ''}${pending}>${number(i)}<code>${current ? '' : highlight(line, language)}</code>${current ? '<span data-kit="code-cursor" aria-hidden="true"></span>' : ''}</li>`
      })
      .join('')}${typing ? `<li${this.line < lines.length ? ' data-pending' : ''}>${number(lines.length)}${this.line < lines.length ? '' : '<span data-kit="code-cursor" data-idle aria-hidden="true"></span>'}</li>` : ''}</ol></div></spec-display><ui-button><button data-kit="code-copy" aria-label="Copy code"></button></ui-button></div>`
    const button = this.querySelector<HTMLElement>('[data-kit="code-copy"]')!
    button.addEventListener('click', () => {
      const reset = () => {
        clearTimeout(this.copied)
        delete button.dataset.copied
        button.ariaLabel = 'Copy code'
      }
      notify('Copied', { description: `${lines.length} lines of ${names[language] || 'code'}` })
      haptic('success')
      button.dataset.copied = ''
      button.ariaLabel = 'Copied'
      clearTimeout(this.copied)
      this.copied = setTimeout(reset, 1400)
      navigator.clipboard.writeText(code).catch(() => {
        reset()
        haptic('error')
        notify("Couldn't copy", { description: "The clipboard wasn't available." })
      })
    })
    this.type()
  }

  type() {
    clearTimeout(this.timer)
    const { line, chars, lines } = this
    if (line >= lines.length || !this.isConnected) return
    const done = chars >= lines[line].length
    this.timer = setTimeout(
      () => {
        const items = this.querySelector('ol')!.children
        if (done) {
          const next = items[++this.line]
          this.chars = 0
          next.removeAttribute('data-pending')
          next.removeAttribute('aria-hidden')
          const cursor = items[line].querySelector<HTMLElement>('[data-kit="code-cursor"]')!
          if (this.line === lines.length) cursor.dataset.idle = ''
          next.append(cursor)
        } else items[line].querySelector('code')!.innerHTML = highlight(lines[line].slice(0, ++this.chars), this.language)
        this.type()
      },
      done ? 320 : 18 + Math.random() * 26,
    )
  }
}

define('spec-code-block', CodeBlock)

export { type Language }
