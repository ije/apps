import '@/components/ui/select'

const rates = ['8', '11.025', '12', '16', '22.05', '24', '32', '37.8', '44.056', '44.1', '47.25', '48', '50', '50.4', '64', '88.2', '96', '176.4', '192', '352.8', '384', '705.6', '768', '2822.4']

export default function mount(root: HTMLElement) {
  root.innerHTML = `
    <spec-select label="Sample rate" default-value="48">
      ${rates.map((rate) => `<option value="${rate}">${rate} kHz</option>`).join('')}
    </spec-select>`
}
