import '@/components/ui/carousel'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-carousel>
      <option>Blog</option>
      <option>Store</option>
      <option>Docs</option>
      <option>Portfolio</option>
    </spec-carousel>`
}
