import { define, Kit } from '@/lib/kit'

class Navbar extends Kit {
  #links!: HTMLElement
  #resize = new ResizeObserver(() => this.#center())
  #current = new MutationObserver(() => this.#center())

  render() {
    this.dataset.kit = 'navbar'
    this.fill(
      `<slot name="brand"></slot><slot name="leading"></slot><nav aria-label="${this.getAttribute('label') ?? 'Pages'}" data-kit="navbar-links"><slot></slot></nav><slot name="end"></slot>`,
    )
    this.#links = this.querySelector('nav')!
    this.querySelector(':scope > a[slot="brand"]')?.setAttribute('data-kit', 'navbar-brand')
    for (const link of this.#links.children) link.setAttribute('data-kit', 'navbar-link')
    for (const link of this.querySelectorAll('a')) link.draggable = false
  }

  connectedCallback() {
    super.connectedCallback()
    this.#resize.observe(this.#links)
    this.#current.observe(this.#links, { subtree: true, attributeFilter: ['aria-current'] })
  }

  disconnectedCallback() {
    this.#resize.disconnect()
    this.#current.disconnect()
  }

  #center() {
    const links = this.#links
    const link = links.querySelector<HTMLElement>('[aria-current]')
    if (!link || links.scrollWidth <= links.clientWidth) return
    links.scrollTo({ left: link.offsetLeft - (links.clientWidth - link.offsetWidth) / 2, behavior: 'smooth' })
  }
}

define('spec-navbar', Navbar)
