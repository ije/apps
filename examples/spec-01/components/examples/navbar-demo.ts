import '@/components/ui/navbar'
import river from '@/photos/river.jpg'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <div data-navbar-demo>
      <img src="${river}" alt="" draggable="false">
      <spec-navbar label="Example pages">
        <button type="button" aria-current="page">Layout</button>
        <button type="button">Routing</button>
        <button type="button">Export</button>
      </spec-navbar>
    </div>`
  const pages = root.querySelectorAll('button')
  for (const page of pages) {
    page.onclick = () => pages.forEach((other) => other.toggleAttribute('aria-current', other === page))
  }
}
