import '@ui-sdk/web/textarea'
import { define, escape, Kit } from '@/lib/kit'

let count = 0

class Textarea extends Kit {
  render() {
    const id = `spec-textarea-${++count}`
    this.dataset.kit = 'field'
    this.innerHTML = `<label data-kit="label" for="${id}">${escape(this.getAttribute('label'))}</label><ui-textarea><textarea data-kit="input" data-multiline id="${id}" placeholder="${escape(this.getAttribute('placeholder'))}"></textarea></ui-textarea>`
  }
}

define('spec-textarea', Textarea)
