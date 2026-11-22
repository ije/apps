import '@/components/ui/toolbar'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-toolbar-group label="Formatting">
      <option value="bold" selected>Bold</option>
      <option value="italic">Italic</option>
      <option value="underline">Underline</option>
    </spec-toolbar-group>`
}
