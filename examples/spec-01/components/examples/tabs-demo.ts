import '@/components/ui/tabs'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-tabs>
      <section value="mix" label="Mix">Levels, pan and sends for every track on the current pattern.</section>
      <section value="edit" label="Edit">Trim, loop and reverse the selected sample. Changes are non-destructive.</section>
      <section value="fx" label="FX">Two effect slots per track: punch-in filters, delay and tape stop.</section>
    </spec-tabs>`
}
