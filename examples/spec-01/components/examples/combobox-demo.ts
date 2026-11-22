import '@/components/ui/combobox'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-combobox label="Framework">
      <option>Next.js</option>
      <option>Nuxt</option>
      <option>SvelteKit</option>
      <option>SolidStart</option>
      <option>Astro</option>
    </spec-combobox>`
}
