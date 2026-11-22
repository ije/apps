import '@/components/ui/menu'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-menu label="More">
      <option>Duplicate</option>
      <option>Rename</option>
      <option>Export</option>
      <option>Delete</option>
    </spec-menu>`
}
