import { define, escape, Kit } from '@/lib/kit'

// WARNING: a real <fieldset>, because its legend sits in the border and is not a grid item of the fieldset.
class Fieldset extends Kit {
  render() {
    this.fill(`<fieldset data-kit="fieldset"><legend data-kit="legend">${escape(this.getAttribute('legend'))}</legend><div data-kit="fieldset-body"><slot></slot></div></fieldset>`)
  }
}

define('spec-fieldset', Fieldset, 'contents')
