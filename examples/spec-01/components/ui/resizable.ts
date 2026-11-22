import '@ui-sdk/web/resizable'
import { crosses, haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

class Resizable extends Kit {
  render() {
    const text = (name: string) => escape(this.getAttribute(name))
    this.innerHTML = `
      <ui-resizable data-kit="resizable" orientation="${this.getAttribute('orientation') ?? 'horizontal'}">
        <ui-resizable-panel default-size="40" min-size="20" data-kit="resizable-panel">${text('left')}</ui-resizable-panel>
        <ui-resizable-handle data-kit="resizable-handle" aria-label="Resize"></ui-resizable-handle>
        <ui-resizable-panel default-size="60" min-size="20" data-kit="resizable-panel">${text('right')}</ui-resizable-panel>
      </ui-resizable>`
    let last = 40
    this.firstElementChild!.addEventListener('layoutchange', (event) => {
      const [size] = (event as CustomEvent<{ value: number[] }>).detail.value
      if (crosses(last, size, 5)) haptic('selection')
      last = size
    })
  }
}

define('spec-resizable', Resizable, 'contents')
