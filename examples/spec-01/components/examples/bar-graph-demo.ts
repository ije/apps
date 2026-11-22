import '@/components/ui/bar-graph'

export default function mount(root: HTMLElement) {
  root.innerHTML = '<div data-bar-graph-demo><spec-bar-graph label="Traffic" segments="16"></spec-bar-graph></div>'
  const graph = root.querySelector('spec-bar-graph')!
  graph.values = [0.32, 0.4, 0.62, 0.78, 0.84, 0.71, 0.66, 0.74, 0.58, 0.44, 0.5, 0.61]
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) return
  const id = setInterval(() => {
    graph.values = graph.values.map((v) => Math.min(1, Math.max(0.08, v + (Math.random() - 0.5) * 0.18)))
  }, 700)
  return () => clearInterval(id)
}
