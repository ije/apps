import '@ui-sdk/web/date-picker'
import '@ui-sdk/web/field'
import '@/components/ui/date-setter'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class DatePicker extends Kit {
  #picker!: HTMLElementTagNameMap['ui-date-picker']
  #setter!: HTMLElement & { value: string }
  #fallback = ''

  render() {
    const label = escape(this.getAttribute('label'))
    this.#fallback = this.getAttribute('default-value') ?? ''
    this.innerHTML = `
      <ui-date-picker default-value="${escape(this.#fallback)}">
        <ui-field data-kit="field">
          <ui-field-label data-kit="label">${label}</ui-field-label>
          <div data-kit="input-group">
            <ui-date-picker-input><input data-kit="input" aria-label="${label}"></ui-date-picker-input>
            <ui-date-picker-trigger><button type="button" data-kit="input-addon" aria-label="Choose date"><spec-icon name="calendar"></spec-icon></button></ui-date-picker-trigger>
          </div>
        </ui-field>
        <ui-date-picker-content data-kit="popover" align="start" side-offset="14">
          <div data-kit="popover-surface" data-size="date-device">
            <spec-date-setter label="${label}" value="${escape(this.#fallback)}"></spec-date-setter>
          </div>
        </ui-date-picker-content>
      </ui-date-picker>`
    this.#picker = this.querySelector('ui-date-picker')!
    this.#setter = this.querySelector('spec-date-setter')!
    this.#picker.addEventListener('valuechange', (event) => {
      const { value, reason } = (event as CustomEvent).detail
      if (reason === 'imperative') return
      const next = typeof value === 'string' ? value : null
      this.#setter.value = next ?? this.#fallback
      this.emit('valuechange', next)
    })
    this.#setter.addEventListener('valuechange', (event) => {
      const { value } = (event as CustomEvent).detail
      this.#picker.value = value
      this.emit('valuechange', value)
    })
  }

  get value() {
    return this.#picker ? (this.#picker.value as string | null) : this.getAttribute('default-value')
  }

  set value(value: string | null) {
    if (!this.#picker) this.setAttribute('default-value', value ?? '')
    else {
      this.#picker.value = value
      this.#setter.value = value ?? this.#fallback
    }
  }
}

define('spec-date-picker', DatePicker, 'contents')
