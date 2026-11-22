import '@ui-sdk/web/scroll-area'
import { define, escape, Kit } from '@/lib/kit'

class ScrollArea extends Kit {
  render() {
    const items = this.options()
    this.innerHTML = `
      <ui-scroll-area data-kit="scroll-area">
        <ui-scroll-area-viewport data-kit="scroll-area-viewport" role="region" aria-label="${escape(this.getAttribute('label'))}">
          <ui-scroll-area-content data-kit="scroll-area-content">
            ${items.map(({ label }) => `<div data-kit="scroll-area-row">${escape(label)}</div>`).join('')}
          </ui-scroll-area-content>
        </ui-scroll-area-viewport>
        <ui-scroll-area-scrollbar data-kit="scroll-area-scrollbar"><ui-scroll-area-thumb data-kit="scroll-area-thumb"></ui-scroll-area-thumb></ui-scroll-area-scrollbar>
      </ui-scroll-area>`
  }
}

define('spec-scroll-area', ScrollArea, 'contents')
