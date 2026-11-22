import '@/components/ui/checkbox'
import '@/components/ui/drawer'
import '@/components/ui/fieldset'
import '@/components/ui/segmented-control'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-drawer trigger="Filters" heading="Filter deployments" description="Show only what you need.">
      <spec-fieldset legend="Environment">
        <spec-segmented label="Environment" default-value="All">
          <option>All</option>
          <option>Production</option>
          <option>Preview</option>
        </spec-segmented>
      </spec-fieldset>
      <spec-fieldset legend="Status">
        <spec-checkbox label="Ready" default-checked></spec-checkbox>
        <spec-checkbox label="Building" default-checked></spec-checkbox>
        <spec-checkbox label="Error"></spec-checkbox>
      </spec-fieldset>
    </spec-drawer>`
}
