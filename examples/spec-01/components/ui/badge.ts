import { define } from '@/lib/kit'

class Badge extends HTMLElement {
  static observedAttributes = ['tone']

  connectedCallback() {
    this.attributeChangedCallback()
  }

  attributeChangedCallback() {
    this.dataset.kit = 'badge'
    this.dataset.tone = this.getAttribute('tone') ?? 'neutral'
  }
}

define('spec-badge', Badge, 'inline')
