import { define, escape, Kit } from '@/lib/kit'

class DevicePlate extends Kit {
  static observedAttributes = ['on']

  render() {
    this.toggleAttribute('data-device-plate', true)
    this.toggleAttribute('data-under', this.hasAttribute('under'))
    this.innerHTML = `<span data-device-power aria-hidden="true"></span><strong>${escape(this.getAttribute('model'))}</strong><span>${escape(this.getAttribute('name'))}</span>`
    this.update()
  }

  update() {
    this.firstElementChild!.toggleAttribute('data-on', this.getAttribute('on') !== 'false')
  }
}

class Device extends Kit {
  static observedAttributes = ['on']
  #resize?: ResizeObserver
  #frame = 0

  render() {
    const name = this.getAttribute('name')!
    this.toggleAttribute('data-device', true)
    this.ariaLabel = name
    if (this.getAttribute('plate') !== 'none') {
      this.insertAdjacentHTML('afterbegin', `<spec-device-plate model="${escape(this.getAttribute('model'))}" name="${escape(name)}"></spec-device-plate>`)
    }
    this.style.maxWidth = 'none'
    this.update()
  }

  update() {
    this.querySelector(':scope > spec-device-plate')?.setAttribute('on', this.getAttribute('on') ?? 'true')
  }

  connectedCallback() {
    super.connectedCallback()
    const page = this.parentElement!
    let width = -1
    const fit = () => {
      // WARNING: zoom changes the height of the page, so only a new width may fit again.
      if (page.clientWidth === width) return
      width = page.clientWidth
      this.style.zoom = ''
      const style = getComputedStyle(page)
      const room = width - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight)
      if (room > 0 && this.offsetWidth > room) this.style.zoom = String(room / this.offsetWidth)
    }
    fit()
    this.#resize = new ResizeObserver(() => {
      cancelAnimationFrame(this.#frame)
      this.#frame = requestAnimationFrame(fit)
    })
    this.#resize.observe(page)
  }

  disconnectedCallback() {
    this.#resize?.disconnect()
    cancelAnimationFrame(this.#frame)
  }
}

const clock = (seconds: number) => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, '0')}`

define('spec-device-plate', DevicePlate)
define('spec-device', Device)

export { clock }
