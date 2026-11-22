import '@ui-sdk/web/carousel'
import '@/components/ui/icon'
import { haptic } from '@/lib/haptics'
import { define, escape, Kit } from '@/lib/kit'

class Carousel extends Kit {
  render() {
    const slides = this.options().map(({ label }) => escape(label))
    this.innerHTML = `
      <ui-carousel data-kit="carousel" aria-label="Templates">
        <ui-carousel-previous><button type="button" data-kit="button" data-tone="secondary" data-shape="round" aria-label="Previous slide"><spec-icon name="prev"></spec-icon></button></ui-carousel-previous>
        <ui-carousel-viewport data-kit="carousel-viewport">
          <ui-carousel-content data-kit="carousel-content">
            ${slides.map((slide) => `<ui-carousel-item data-kit="carousel-item"><div data-kit="tile">${slide}</div></ui-carousel-item>`).join('')}
          </ui-carousel-content>
        </ui-carousel-viewport>
        <ui-carousel-next><button type="button" data-kit="button" data-tone="secondary" data-shape="round" aria-label="Next slide"><spec-icon name="next"></spec-icon></button></ui-carousel-next>
        <ui-carousel-indicator-group data-kit="carousel-indicators" aria-label="Choose a template">
          ${slides.map((slide, i) => `<ui-carousel-indicator page="${i + 1}"><button type="button" aria-label="${slide}" data-kit="carousel-indicator"></button></ui-carousel-indicator>`).join('')}
        </ui-carousel-indicator-group>
      </ui-carousel>`
    this.firstElementChild!.addEventListener('pagechange', () => haptic('selection'))
  }
}

define('spec-carousel', Carousel, 'contents')
