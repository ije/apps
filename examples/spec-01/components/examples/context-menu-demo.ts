import '@/components/ui/context-menu'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-context-menu-area label="Right-click" touch-label="Long-press">
      <option>Copy</option>
      <option>Paste</option>
      <option>Duplicate</option>
      <option>Delete</option>
    </spec-context-menu-area>`
}
