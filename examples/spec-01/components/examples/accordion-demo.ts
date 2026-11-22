import '@/components/ui/accordion'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-accordion>
      <section value="storage" label="Storage">64 GB internal. Projects back up to your computer over USB-C.</section>
      <section value="battery" label="Battery">About 16 hours of playback. Charges to 80% in an hour.</section>
      <section value="updates" label="Updates">New features arrive as free updates, installed from the device.</section>
    </spec-accordion>`
}
