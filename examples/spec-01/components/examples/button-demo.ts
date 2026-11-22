import '@/components/ui/button'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-button tone="primary">Play</spec-button>
    <spec-button>Stop</spec-button>
    <spec-button round aria-label="Record"><span data-kit="record-dot" aria-hidden="true"></span></spec-button>`
}
