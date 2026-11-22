import { define, Kit } from '@/lib/kit'

class BarGraph extends Kit {
  static observedAttributes = ['label', 'segments']
  #values: number[] = []

  render() {
    this.dataset.kit = 'bar-graph'
    this.setAttribute('role', 'img')
    this.update()
    this.values = this.#values
  }

  update() {
    this.ariaLabel = this.getAttribute('label')
    this.style.setProperty('--segments', this.getAttribute('segments') ?? '18')
  }

  get values() {
    return this.#values
  }

  set values(values: number[]) {
    this.#values = values
    if (!this.rendered) return
    if (this.children.length !== values.length) {
      this.style.setProperty('--columns', String(values.length))
      this.innerHTML = '<span data-kit="bar-graph-bar"></span>'.repeat(values.length)
    }
    values.forEach((value, i) => (this.children[i] as HTMLElement).style.setProperty('--v', String(Math.min(1, Math.max(0, value)))))
  }
}

define('spec-bar-graph', BarGraph)

declare global {
  interface HTMLElementTagNameMap {
    'spec-bar-graph': BarGraph
  }
}
