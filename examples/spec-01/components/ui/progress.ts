import '@ui-sdk/web/progress'
import { define, Kit } from '@/lib/kit'

class Progress extends Kit {
  static observedAttributes = ['label', 'value']

  render() {
    this.dataset.kit = 'meter'
    this.innerHTML = `
      <ui-progress style="display: contents">
        <div data-kit="row">
          <ui-progress-label data-kit="label"></ui-progress-label>
          <ui-progress-value data-kit="value"></ui-progress-value>
        </div>
        <ui-progress-track data-kit="meter-track"><ui-progress-indicator data-kit="meter-indicator"></ui-progress-indicator></ui-progress-track>
      </ui-progress>`
    this.update()
  }

  update() {
    this.querySelector('ui-progress-label')!.textContent = this.getAttribute('label')
    this.firstElementChild!.setAttribute('value', this.getAttribute('value') ?? '0')
  }
}

define('spec-progress', Progress)
