const displays = new CSSStyleSheet()
document.adoptedStyleSheets.push(displays)

// Skin styles are not in a layer, so they win over this default display.
export function define(tag: string, element: CustomElementConstructor, display = 'block') {
  displays.insertRule(`@layer kit { ${tag} { display: ${display} } }`)
  customElements.define(tag, element)
}

export const escape = (text: unknown) => String(text ?? '').replace(/[&<>"]/g, (char) => `&#${char.charCodeAt(0)};`)

export class Kit extends HTMLElement {
  rendered = false

  connectedCallback() {
    if (this.rendered) return
    this.rendered = true
    this.render()
  }

  attributeChangedCallback() {
    if (this.rendered) this.update()
  }

  render() {}

  update() {}

  // WARNING: reads the children, so the element must connect after they are parsed, as in a module script.
  fill(html: string) {
    const children = [...this.childNodes]
    this.innerHTML = html
    for (const slot of this.querySelectorAll('slot')) {
      const name = slot.getAttribute('name')
      slot.replaceWith(...children.filter((child) => ((child as Element).getAttribute?.('slot') ?? null) === name))
    }
  }

  // WARNING: call before fill(), which removes the <option> children.
  options() {
    return [...this.querySelectorAll<HTMLOptionElement>(':scope > option')].map(({ value, label }) => ({ value, label }))
  }

  // Moves the attributes that are not options of the element to the part that the skin styles.
  pass(part: Element, ...options: string[]) {
    for (const { name, value } of [...this.attributes]) {
      if (name === 'slot' || options.includes(name)) continue
      part.setAttribute(name, value)
      this.removeAttribute(name)
    }
  }

  emit(type: string, value?: unknown) {
    return this.dispatchEvent(new CustomEvent(type, { detail: { value }, cancelable: true }))
  }
}
