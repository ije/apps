import '@/components/ui/slide-switch'

const options = (values: string[]) => values.map((value) => `<option>${value}</option>`).join('')

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <div data-switch-demo>
      <spec-slide-switch label="Output" orientation="vertical" side="start" default-value="Cue">${options(['Main', 'Cue', 'Both'])}</spec-slide-switch>
      <spec-slide-switch label="Quality" default-value="Mid">${options(['Lo', 'Mid', 'Hi'])}</spec-slide-switch>
      <spec-slide-switch label="Input" orientation="vertical" default-value="Line">${options(['Line', 'Mic', 'USB'])}</spec-slide-switch>
    </div>`
}
