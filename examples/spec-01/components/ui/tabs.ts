import '@ui-sdk/web/tabs'
import { define, escape, Kit } from '@/lib/kit'

type TabsElement = HTMLElement & { value: string | null }

class Tabs extends Kit {
  static observedAttributes = ['value']
  #tabs!: TabsElement

  render() {
    const items = [...this.querySelectorAll<HTMLElement>(':scope > section')]
    const value = this.getAttribute('value') ?? items[0]?.getAttribute('value') ?? ''
    this.innerHTML = `
      <ui-tabs data-kit="tabs" default-value="${escape(value)}">
        <ui-tabs-list data-kit="tabs-list" aria-label="${escape(this.getAttribute('label') ?? 'Sections')}">
          <ui-tabs-indicator data-kit="tabs-indicator"></ui-tabs-indicator>
          ${items.map((item) => `<ui-tabs-tab value="${escape(item.getAttribute('value'))}"><button type="button" data-kit="tab">${escape(item.getAttribute('label'))}</button></ui-tabs-tab>`).join('')}
        </ui-tabs-list>
        ${items.map((item) => `<ui-tabs-panel value="${escape(item.getAttribute('value'))}" data-kit="tab-panel"></ui-tabs-panel>`).join('')}
      </ui-tabs>`
    this.#tabs = this.firstElementChild as TabsElement
    this.querySelectorAll('ui-tabs-panel').forEach((panel, i) => panel.append(...items[i].childNodes))
    this.#tabs.addEventListener('valuechange', (event) => {
      const { value, reason } = (event as CustomEvent).detail
      if (reason !== 'imperative' && !this.emit('valuechange', String(value))) event.preventDefault()
    })
  }

  update() {
    const value = this.getAttribute('value')
    if (value !== null) this.value = value
  }

  get value() {
    return this.#tabs?.value ?? this.getAttribute('value') ?? ''
  }

  set value(value: string) {
    if (!this.#tabs) this.setAttribute('value', value)
    else if (this.#tabs.value !== value) this.#tabs.value = value
  }
}

define('spec-tabs', Tabs, 'contents')
