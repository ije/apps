import { compile } from 'svelte/compiler'

export default {
  load({ filename, content }: { filename: string; content: string }) {
    const { js } = compile(content, { filename, generate: 'client', css: 'injected' })
    return { type: 'js', code: js.code, map: js.map }
  },
}
