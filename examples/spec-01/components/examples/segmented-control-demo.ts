import '@/components/ui/segmented-control'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-segmented label="Pattern" default-value="1">
      <option>1</option>
      <option>2</option>
      <option>3</option>
      <option>4</option>
    </spec-segmented>`
}
