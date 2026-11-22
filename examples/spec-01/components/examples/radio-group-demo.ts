import '@/components/ui/radio-group'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-radio-group label="Sync source" default-value="Internal">
      <option>Internal</option>
      <option>MIDI</option>
      <option>USB</option>
    </spec-radio-group>`
}
