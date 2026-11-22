import '@ui-sdk/web/accordion'
import { define, escape, Kit } from '@/lib/kit'

class Accordion extends Kit {
  render() {
    const items = [...this.querySelectorAll<HTMLElement>(':scope > section')]
    this.innerHTML = `
      <ui-accordion data-kit="accordion" default-value="${escape(items[0]?.getAttribute('value'))}">
        ${items
          .map(
            (item) => `
              <ui-accordion-item value="${escape(item.getAttribute('value'))}" data-kit="accordion-item">
                <ui-accordion-header><h3 data-kit="accordion-header"><ui-accordion-trigger><button type="button" data-kit="accordion-trigger">${escape(item.getAttribute('label'))}<span data-kit="accordion-led" aria-hidden="true"></span></button></ui-accordion-trigger></h3></ui-accordion-header>
                <ui-accordion-panel data-kit="accordion-panel"><p></p></ui-accordion-panel>
              </ui-accordion-item>`,
          )
          .join('')}
      </ui-accordion>`
    this.querySelectorAll('ui-accordion-panel > p').forEach((p, i) => p.append(...items[i].childNodes))
  }
}

define('spec-accordion', Accordion, 'contents')
