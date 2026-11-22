import '@ui-sdk/web/listbox'
import { define, escape, Kit } from '@/lib/kit'

type Column<T> = { key: string; label: string; render: (row: T) => unknown; align?: 'start' | 'end'; width?: string }

class ItemList<T = any> extends Kit {
  #rows: T[] = []
  #columns: Column<T>[] = []
  #value: string | null = null
  #progress: number | undefined
  #body!: HTMLElementTagNameMap['ui-listbox']
  #current: HTMLElement | null = null
  rowKey = (row: T) => String(row)

  render() {
    this.dataset.kit = 'item-list'
    this.innerHTML = `<div data-kit="item-list-head" aria-hidden="true"></div><ui-listbox data-kit="item-list-body" activation="manual" aria-label="${escape(this.getAttribute('label'))}"></ui-listbox>`
    this.#body = this.querySelector('ui-listbox')!
    this.#body.addEventListener('valuechange', (event) => {
      const next = event.detail.value as string | null
      if (event.detail.reason === 'imperative' || !next) return
      this.#select(next)
      this.emit('valuechange', next)
    })
    this.#fillColumns()
  }

  get columns() {
    return this.#columns
  }

  set columns(columns: Column<T>[]) {
    this.#columns = columns
    if (this.rendered) this.#fillColumns()
  }

  get rows() {
    return this.#rows
  }

  set rows(rows: T[]) {
    this.#rows = rows
    if (this.rendered) this.#fillRows()
  }

  get value() {
    return this.#value
  }

  set value(value: string | null) {
    if (value === this.#value) return
    if (this.rendered) this.#body.value = value
    this.#select(value)
  }

  get progress() {
    return this.#progress
  }

  set progress(progress: number | undefined) {
    this.#progress = progress
    this.#paint()
  }

  #fillColumns() {
    const columns = this.#columns
    this.style.setProperty('--columns', columns.map((column) => column.width ?? 'auto').join(' '))
    this.firstElementChild!.innerHTML = columns.map((column) => `<span${column.align ? ` data-align="${column.align}"` : ''}>${escape(column.label)}</span>`).join('')
    this.#fillRows()
  }

  #fillRows() {
    this.#body.innerHTML = this.#rows
      .map((row) => {
        const cells = this.#columns.map((column) => `<span${column.align ? ` data-align="${column.align}"` : ''}>${escape(column.render(row))}</span>`).join('')
        return `<ui-listbox-item value="${escape(this.rowKey(row))}" data-kit="item-list-row">${cells}</ui-listbox-item>`
      })
      .join('')
    this.#body.value = this.#value
    this.#current = null
    this.#select(this.#value)
  }

  #select(value: string | null) {
    this.#value = value
    if (!this.rendered) return
    this.#current?.removeAttribute('data-progress')
    this.#current?.style.removeProperty('--progress')
    this.#current = value === null ? null : [...this.#body.children].find((row) => row.getAttribute('value') === value) as HTMLElement ?? null
    this.#paint()
  }

  #paint() {
    const row = this.#current
    if (!row) return
    if (this.#progress === undefined) {
      row.removeAttribute('data-progress')
      row.style.removeProperty('--progress')
      return
    }
    row.setAttribute('data-progress', '')
    row.style.setProperty('--progress', String(this.#progress))
  }
}

define('spec-item-list', ItemList)

declare global {
  interface HTMLElementTagNameMap {
    'spec-item-list': ItemList
  }
}

export { type Column }
