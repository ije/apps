import '@ui-sdk/web/otp-input'
import { define, escape, Kit } from '@/lib/kit'

class OTP extends Kit {
  render() {
    const label = escape(this.getAttribute('label'))
    const length = Number(this.getAttribute('length') ?? 6)
    const half = length / 2
    const group = (from: number) =>
      `<ui-otp-input-group data-kit="otp-group">${Array.from({ length: half }, (_, index) => `<ui-otp-input-slot data-kit="otp-slot" index="${from + index}"></ui-otp-input-slot>`).join('')}</ui-otp-input-group>`
    this.dataset.kit = 'field'
    this.innerHTML = `<label data-kit="label">${label}</label><ui-otp-input data-kit="otp" max-length="${length}" aria-label="${label}">${group(0)}<ui-otp-input-separator data-kit="otp-separator">–</ui-otp-input-separator>${group(half)}</ui-otp-input>`
    this.firstElementChild!.addEventListener('click', () => this.querySelector<HTMLInputElement>('ui-otp-input input')?.focus())
  }
}

define('spec-otp', OTP)
