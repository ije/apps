import { define, Kit } from '@/lib/kit'

class Alert extends Kit {
  static observedAttributes = ['tone', 'heading']

  render() {
    this.dataset.kit = 'alert'
    this.fill(
      '<div data-kit="legend"><span data-kit="alert-led" aria-hidden="true"></span><strong data-kit="alert-title"></strong></div><p data-kit="alert-description"><slot></slot></p>',
    )
    this.update()
  }

  update() {
    const tone = this.getAttribute('tone') ?? 'info'
    this.dataset.tone = tone
    this.setAttribute('role', tone === 'danger' ? 'alert' : 'status')
    this.querySelector('[data-kit="alert-title"]')!.textContent = this.getAttribute('heading')
  }
}

define('spec-alert', Alert)
