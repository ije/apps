import '@/components/ui/lcd'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <div data-lcd-demo>
      <spec-lcd tag="Tempo"><spec-lcd-digits value="128.0"></spec-lcd-digits></spec-lcd>
      <spec-lcd tag="Status">Synced to MIDI clock</spec-lcd>
      <spec-lcd tag="Battery" size="sm"><spec-lcd-digits value="82%"></spec-lcd-digits></spec-lcd>
      <spec-lcd tag="Timer" size="lg"><spec-lcd-digits value="04:32"></spec-lcd-digits></spec-lcd>
    </div>`
}
