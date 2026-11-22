import { notify } from '@/components/ui/toaster'

declare global {
  interface Window {
    app?: { save(name: string, png: string): void }
  }
}

export function save(name: string, canvas: HTMLCanvasElement) {
  const png = canvas.toDataURL('image/png')
  if (window.app) {
    window.app.save(name, png)
    return notify('Saved', { description: `${name} is in Downloads.` })
  }
  const link = document.createElement('a')
  link.download = name
  link.href = png
  link.click()
}
