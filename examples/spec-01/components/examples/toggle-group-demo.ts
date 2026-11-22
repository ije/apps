import '@/components/ui/toggle-group'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-toggle-group label="Alignment" default-value="left">
      <option>left</option>
      <option>center</option>
      <option>right</option>
    </spec-toggle-group>`
}
