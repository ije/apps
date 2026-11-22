import '@/components/ui/flat-knob'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-flat-knob label="Tone" color="blue" default-value="30"></spec-flat-knob>
    <spec-flat-knob label="Drive" color="orange" default-value="72"></spec-flat-knob>
    <spec-flat-knob label="Space" color="white" default-value="50"></spec-flat-knob>
    <spec-flat-knob label="Mix" color="green" default-value="18"></spec-flat-knob>`
}
