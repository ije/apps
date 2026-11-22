import { define, escape, Kit } from '@/lib/kit'

class Display extends Kit {
  render() {
    const label = this.getAttribute('label')
    const aspect = this.getAttribute('aspect') ?? '16 / 10'
    this.dataset.kit = 'display'
    this.fill(
      `<div data-kit="display-bezel"><div data-kit="display-screen"><slot></slot></div></div>${label ? `<figcaption data-kit="display-label">${escape(label)}</figcaption>` : ''}`,
    )
    if (aspect !== 'auto') this.querySelector<HTMLElement>('[data-kit="display-screen"]')!.style.aspectRatio = aspect
  }
}

define('spec-display', Display)
