import '@ui-sdk/web/separator'
import { define, Kit } from '@/lib/kit'

class Separator extends Kit {
  render() {
    this.innerHTML = '<ui-separator data-kit="separator"></ui-separator>'
  }
}

define('spec-separator', Separator, 'contents')
