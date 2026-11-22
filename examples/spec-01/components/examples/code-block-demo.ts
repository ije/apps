import '@/components/ui/code-block'
import { escape } from '@/lib/kit'

const code = `import { createSlider } from "@ui-sdk/core"

// A fader that snaps to the beat.
const fader = createSlider(el, {
  min: 0,
  max: 127,
  step: 1,
  onValueChange: (value) => send("cc", 7, value),
})`

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-code-block language="ts" code="${escape(code)}"></spec-code-block>`
}
