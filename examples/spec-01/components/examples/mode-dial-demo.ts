import '@/components/ui/mode-dial'

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <div data-mode-dial-row>
      <spec-mode-dial label="Effort" default-value="high">
        ${['low', 'medium', 'high', 'xhigh', 'max'].map((n) => `<option value="${n}">${n}</option>`).join('')}
      </spec-mode-dial>
      <spec-mode-dial label="Bank" default-value="3" sweep="180">
        ${['1', '2', '3', '4', '5'].map((n) => `<option value="${n}">${n}</option>`).join('')}
      </spec-mode-dial>
    </div>`
}
