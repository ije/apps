import { installHardware } from '@/lib/install'
import { define } from '@/lib/kit'

class Hardware extends HTMLElement {
  static observedAttributes = ['key-color']

  connectedCallback() {
    installHardware()
    this.dataset.skin = 'hardware'
  }

  attributeChangedCallback(_name: string, _old: string | null, color: string | null) {
    const [l, c, h] = color?.trim().split(/\s+/) ?? []
    for (const [channel, value] of Object.entries({ l, c, h })) {
      if (value) this.style.setProperty(`--theme-${channel}`, value)
      else this.style.removeProperty(`--theme-${channel}`)
    }
  }
}

define('spec-hardware', Hardware)
