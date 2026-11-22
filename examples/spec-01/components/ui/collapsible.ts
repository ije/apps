import '@ui-sdk/web/collapsible'
import { define, escape, Kit } from '@/lib/kit'

type CollapsibleElement = HTMLElement & { open: boolean; api: { toggle(options: object): boolean } }

class Collapsible extends Kit {
  render() {
    this.fill(`
      <ui-collapsible data-kit="collapsible">
        <button type="button" data-kit="accordion-trigger" aria-expanded="false">${escape(this.getAttribute('heading'))}</button>
        <ui-collapsible-panel data-kit="accordion-panel" hidden><p><slot></slot></p></ui-collapsible-panel>
      </ui-collapsible>`)
    // WARNING: the skin wants the button as a direct child of the root, so it can't sit in <ui-collapsible-trigger>. It is wired by hand.
    const root = this.firstElementChild as CollapsibleElement
    const button = root.firstElementChild as HTMLButtonElement
    const panel = root.lastElementChild!
    panel.id ||= `${(button.id = `collapsible-${Math.random().toString(36).slice(2)}`)}-panel`
    button.setAttribute('aria-controls', panel.id)
    button.onclick = (event) => root.api.toggle({ reason: 'trigger-press', event })
    root.addEventListener('openchange', (event) => {
      if (event.defaultPrevented) return
      button.ariaExpanded = String((event as CustomEvent).detail.value)
    })
  }
}

define('spec-collapsible', Collapsible, 'contents')
