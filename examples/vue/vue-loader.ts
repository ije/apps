import { createHash } from 'node:crypto'
import { compileScript, compileStyle, parse } from 'vue/compiler-sfc'

// Handles only components with <script setup>.
export default {
  load({ filename, content }: { filename: string; content: string }) {
    const { descriptor, errors } = parse(content, { filename })
    if (errors.length) throw errors[0]
    const id = createHash('sha256').update(filename).digest('hex').slice(0, 8)
    const script = compileScript(descriptor, { id, inlineTemplate: true, genDefaultAs: '_sfc_main' })
    const css = descriptor.styles
      .map(style => compileStyle({ id: `data-v-${id}`, filename, source: style.content, scoped: style.scoped }).code)
      .join('\n')
    let code = script.content
    if (descriptor.styles.some(style => style.scoped)) code += `\n_sfc_main.__scopeId = 'data-v-${id}'`
    if (css) code += `\ndocument.head.append(Object.assign(document.createElement('style'), { textContent: ${JSON.stringify(css)} }))`
    return { type: script.lang === 'ts' ? 'ts' : 'js', code: `${code}\nexport default _sfc_main`, map: script.map }
  },
}
