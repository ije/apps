import '@ui-sdk/web/command'
import '@ui-sdk/web/dialog'
import '@/components/ui/icon'
import { define, escape, Kit } from '@/lib/kit'

class CommandPalette extends Kit {
  render() {
    const commands = this.options()
    this.innerHTML = `
      <ui-dialog style="display: contents">
        <ui-dialog-trigger><button data-kit="button" data-tone="secondary"><spec-icon name="search"></spec-icon><span data-kit="trigger-text">Search…</span><kbd data-kit="kbd">⌘K</kbd></button></ui-dialog-trigger>
        <ui-dialog-backdrop data-kit="backdrop" hidden></ui-dialog-backdrop>
        <ui-dialog-content data-kit="dialog" hidden>
          <div data-kit="dialog-surface" data-size="command">
            <ui-dialog-title data-kit="visually-hidden">Command palette</ui-dialog-title>
            <ui-command data-kit="command">
              <ui-command-input><input data-kit="input" aria-label="Search commands" placeholder="Type a command…"></ui-command-input>
              <ui-command-empty data-kit="empty">No results.</ui-command-empty>
              <ui-command-list data-kit="command-list">
                ${commands.map(({ value, label }) => `<ui-command-item data-kit="option" value="${escape(value)}">${escape(label)}</ui-command-item>`).join('')}
              </ui-command-list>
            </ui-command>
          </div>
        </ui-dialog-content>
      </ui-dialog>`
  }
}

define('spec-command-palette', CommandPalette, 'contents')
