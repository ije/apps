import '@ui-sdk/web/toggle'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class ToggleGroup extends Kit {
  group!: HTMLElement & { value: string[] }

  get value() {
    return this.group ? (this.group.value[0] ?? '') : (this.getAttribute('default-value') ?? '')
  }

  set value(value: string) {
    if (this.group) this.group.value = value ? [value] : []
    else this.setAttribute('default-value', value)
  }

  render() {
    const options = this.options()
    this.innerHTML = `<ui-toggle-group data-kit="toolbar" aria-label="${escape(this.getAttribute('label'))}" default-value="${escape(this.getAttribute('default-value'))}">${options
      .map(
        ({ value, label }) =>
          `<ui-toggle value="${escape(value)}"><button data-kit="toggle" aria-label="${escape(label || value)}"><spec-icon name="align-${escape(value)}"></spec-icon></button></ui-toggle>`,
      )
      .join('')}</ui-toggle-group>`
    this.group = this.firstElementChild as typeof this.group
    this.group.addEventListener('valuechange', (event) => {
      if (!this.emit('valuechange', (event as CustomEvent).detail.value[0] ?? '')) event.preventDefault()
    })
  }
}

define('spec-toggle-group', ToggleGroup, 'contents')
