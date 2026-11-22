import '@ui-sdk/web/button'
import { define, Kit } from '@/lib/kit'

class Button extends Kit {
  render() {
    this.fill(
      `<ui-button><button data-kit="button" data-tone="${this.getAttribute('tone') ?? 'secondary'}"><slot></slot></button></ui-button>`,
    )
    const button = this.querySelector('button')!
    if (this.hasAttribute('round')) button.dataset.shape = 'round'
    this.pass(button, 'tone', 'round')
  }
}

define('spec-button', Button, 'contents')
