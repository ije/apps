import '@ui-sdk/web/slider'
import { crosses, haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

class Slider extends Kit {
  static observedAttributes = ['value', 'min', 'max', 'step']
  #slider?: HTMLElementTagNameMap['ui-slider']

  get value() {
    return (this.#slider?.value as number | undefined) ?? Number(this.getAttribute('value') ?? this.getAttribute('default-value') ?? this.getAttribute('min') ?? 0)
  }

  set value(value: number) {
    if (this.#slider) this.#slider.value = value
    else this.setAttribute('value', String(value))
  }

  render() {
    const label = escape(this.getAttribute('label'))
    this.dataset.kit = 'field'
    this.innerHTML = `
      <ui-slider data-kit="slider"${this.hasAttribute('default-value') ? ` default-value="${escape(this.getAttribute('default-value'))}"` : ''}>
        <div data-kit="row">
          <label data-kit="label">${label}</label>
          <ui-slider-value data-kit="value"></ui-slider-value>
        </div>
        <ui-slider-control data-kit="slider-control">
          <span data-kit="slider-scale" aria-hidden="true"></span>
          <ui-slider-track data-kit="slider-track">
            <ui-slider-indicator data-kit="slider-indicator"></ui-slider-indicator>
            <ui-slider-thumb data-kit="slider-thumb" aria-label="${label}"></ui-slider-thumb>
          </ui-slider-track>
        </ui-slider-control>
      </ui-slider>`
    const slider = (this.#slider = this.querySelector('ui-slider')!)
    this.update()
    slider.addEventListener('valuechange', (event) => {
      const next = event.detail.value as number
      const lo = Number(this.getAttribute('min') ?? 0)
      if (crosses((slider.value as number) - lo, next - lo, (Number(this.getAttribute('max') ?? 100) - lo) / 20)) haptic('selection')
      this.emit('valuechange', next)
    })
  }

  update() {
    for (const name of Slider.observedAttributes) {
      const value = this.getAttribute(name)
      if (value !== null) this.#slider!.setAttribute(name, value)
      else this.#slider!.removeAttribute(name)
    }
  }
}

define('spec-slider', Slider)
