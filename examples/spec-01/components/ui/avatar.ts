import '@ui-sdk/web/avatar'
import { define, escape, Kit } from '@/lib/kit'

class Avatar extends Kit {
  static observedAttributes = ['name', 'size']

  render() {
    this.innerHTML = '<ui-avatar data-kit="avatar"><ui-avatar-fallback data-kit="avatar-fallback"></ui-avatar-fallback></ui-avatar>'
    this.update()
  }

  update() {
    const name = this.getAttribute('name') ?? ''
    const size = this.getAttribute('size')
    const avatar = this.firstElementChild as HTMLElement
    if (size) avatar.dataset.size = size
    else delete avatar.dataset.size
    let hash = 2166136261
    for (const char of name) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619)
    let dots = ''
    for (let row = 0; row < 7; row++) {
      for (let col = 0; col < 7; col++) {
        const half = col < 4 ? col : 6 - col
        const bit = (hash >>> ((row * 4 + half) % 31)) & 1
        dots += Math.hypot(row - 3, col - 3) > 3.2 ? '<i data-off-lens></i>' : bit || (row === 3 && half === 3) ? '<i data-on></i>' : '<i></i>'
      }
    }
    avatar.firstElementChild!.innerHTML = `<span data-kit="avatar-matrix" aria-hidden="true">${dots}</span><span data-kit="visually-hidden">${escape(name)}</span>`
  }
}

define('spec-avatar', Avatar, 'contents')
