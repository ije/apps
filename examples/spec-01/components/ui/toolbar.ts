import '@ui-sdk/web/toggle'
import '@ui-sdk/web/toolbar'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class ToolbarGroup extends Kit {
  render() {
    const toggles = [...this.querySelectorAll<HTMLOptionElement>(':scope > option')]
      .map(
        ({ value, label, defaultSelected }) =>
          `<ui-toggle${defaultSelected ? ' default-pressed' : ''}><button data-kit="toggle" aria-label="${escape(label)}"><spec-icon name="${escape(value)}"></spec-icon></button></ui-toggle>`,
      )
      .join('')
    this.innerHTML = `<ui-toolbar data-kit="toolbar" aria-label="${escape(this.getAttribute('label'))}">${toggles}<ui-toolbar-separator data-kit="separator"></ui-toolbar-separator><ui-toolbar-button><button data-kit="toggle" aria-label="Clear formatting"><spec-icon name="erase"></spec-icon></button></ui-toolbar-button></ui-toolbar>`
  }
}

define('spec-toolbar-group', ToolbarGroup, 'contents')
