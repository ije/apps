import '@/components/ui/menubar'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-menubar>
      <optgroup label="File">
        <option>New</option>
        <option>Open</option>
        <option>Save</option>
      </optgroup>
      <optgroup label="View">
        <option checkable>Sidebar</option>
        <option checkable>Minimap</option>
      </optgroup>
      <optgroup label="Help">
        <option>Docs</option>
        <option>Shortcuts</option>
      </optgroup>
    </spec-menubar>`
}
