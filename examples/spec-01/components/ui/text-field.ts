import '@ui-sdk/web/input'
import { define, escape, Kit } from '@/lib/kit'

let count = 0

class TextField extends Kit {
  render() {
    const id = `spec-text-field-${++count}`
    this.dataset.kit = 'field'
    this.innerHTML = `<label data-kit="label" for="${id}">${escape(this.getAttribute('label'))}</label><ui-input><input data-kit="input" id="${id}" value="${escape(this.getAttribute('default-value'))}" spellcheck="false"></ui-input>`
  }
}

define('spec-text-field', TextField)
