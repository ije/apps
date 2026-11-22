import '@ui-sdk/web/radio'
import { define, escape, Kit } from '@/lib/kit'

class RadioGroup extends Kit {
  render() {
    const options = this.options()
    const label = escape(this.getAttribute('label'))
    this.dataset.kit = 'field'
    this.innerHTML = `
      <label data-kit="label">${label}</label>
      <ui-radio-group data-kit="radio-group" aria-label="${label}" default-value="${escape(this.getAttribute('default-value'))}">
        ${options.map(({ value, label }) => `<label data-kit="check" data-value="${escape(value)}"><ui-radio data-kit="radio" value="${escape(value)}"><ui-radio-indicator data-kit="radio-indicator" keep-mounted></ui-radio-indicator></ui-radio><span data-kit="label">${escape(label)}</span></label>`).join('')}
      </ui-radio-group>`
    this.querySelector('ui-radio-group')!.addEventListener('valuechange', (event) => {
      if (event.detail.value) this.emit('valuechange', String(event.detail.value))
    })
  }
}

define('spec-radio-group', RadioGroup)
