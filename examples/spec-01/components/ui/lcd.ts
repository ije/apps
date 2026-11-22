import { define, Kit } from '@/lib/kit'

class Lcd extends Kit {
  static observedAttributes = ['tag']

  render() {
    this.dataset.kit = 'lcd'
    this.dataset.size = this.getAttribute('size') ?? 'md'
    this.fill('<div data-kit="lcd-body"><slot></slot></div>')
    this.update()
  }

  update() {
    const text = this.getAttribute('tag')
    let tag = this.querySelector(':scope > [data-kit="lcd-tag"]')
    if (!text) return tag?.remove()
    if (!tag) this.prepend((tag = document.createElement('span')))
    tag.setAttribute('data-kit', 'lcd-tag')
    tag.textContent = text
  }
}

class LcdDigits extends HTMLElement {
  static observedAttributes = ['value', 'ghost']

  attributeChangedCallback() {
    if (!this.firstChild) {
      this.dataset.kit = 'lcd-digits'
      this.innerHTML = '<span data-kit="lcd-ghost" aria-hidden="true"></span><span data-kit="lcd-lit"></span>'
    }
    const value = this.getAttribute('value') ?? ''
    this.firstChild!.textContent = this.getAttribute('ghost') ?? value.replace(/[0-9]/g, '8')
    this.lastChild!.textContent = value
  }
}

define('spec-lcd', Lcd)
define('spec-lcd-digits', LcdDigits, 'inline')
