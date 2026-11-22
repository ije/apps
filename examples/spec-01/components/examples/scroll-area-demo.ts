import '@/components/ui/scroll-area'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-scroll-area label="Tags">
      ${['v2.14.0', 'v2.13.4', 'v2.13.3', 'v2.13.2', 'v2.13.1', 'v2.13.0', 'v2.12.9', 'v2.12.8'].map((tag) => `<option>${tag}</option>`).join('')}
    </spec-scroll-area>`
}
