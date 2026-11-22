import '@ui-sdk/web/number-input'
import '@/components/ui/icon'
import { haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

let count = 0

class Stepper extends Kit {
  render() {
    const id = `spec-stepper-${++count}`
    const label = escape(this.getAttribute('label'))
    this.innerHTML = `
      <ui-number-input default-value="${escape(this.getAttribute('default-value'))}" min="0" max="999">
        <div data-kit="field">
          <label data-kit="label" for="${id}">${label}</label>
          <ui-number-input-group data-kit="stepper">
            <ui-number-input-decrement><button data-kit="button" data-tone="secondary" aria-label="Decrease"><spec-icon name="minus"></spec-icon></button></ui-number-input-decrement>
            <ui-number-input-control><input data-kit="stepper-input" id="${id}" aria-label="${label}"></ui-number-input-control>
            <ui-number-input-increment><button data-kit="button" data-tone="secondary" aria-label="Increase"><spec-icon name="plus"></spec-icon></button></ui-number-input-increment>
          </ui-number-input-group>
        </div>
      </ui-number-input>`
    this.querySelector('ui-number-input')!.addEventListener('valuechange', () => haptic('selection'))
  }
}

define('spec-stepper', Stepper, 'contents')
