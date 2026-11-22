import { define, escape, Kit } from '@/lib/kit'

class Breadcrumb extends Kit {
  render() {
    const items = this.options()
    this.setAttribute('role', 'navigation')
    this.setAttribute('aria-label', 'Breadcrumb')
    this.innerHTML = `<ol data-kit="breadcrumb">${items
      .map(({ label }, index) =>
        index === items.length - 1
          ? `<li><span data-kit="breadcrumb-current" aria-current="page">${escape(label)}</span></li>`
          : `<li><a data-kit="breadcrumb-link" tabindex="0">${escape(label)}</a></li>`,
      )
      .join('')}</ol>`
  }
}

define('spec-breadcrumb', Breadcrumb)
