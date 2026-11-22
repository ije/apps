import '@/components/ui/resizable'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <div data-resizable-demo>
      <spec-resizable left="Files" right="Editor"></spec-resizable>
      <spec-resizable left="Preview" right="Console" orientation="vertical"></spec-resizable>
    </div>`
}
