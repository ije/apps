// Client-side routing for the single page. Paths match diffshub.com, like /owner/repo/pull/123.
export function navigate(href: string, replace = false): void {
  history[replace ? 'replaceState' : 'pushState'](null, '', href)
  dispatchEvent(new Event('popstate'))
}

export function subscribeLocation(listener: () => void): () => void {
  addEventListener('popstate', listener)
  return () => removeEventListener('popstate', listener)
}
