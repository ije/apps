import { FileDiff, preloadHighlighter } from '@pierre/diffs'
import type { Editor } from '@pierre/diffs/edit'

const oldFile = {
  name: 'src/reviews.ts',
  contents: `import { readFile } from 'node:fs/promises'
import type { Review, ReviewState } from './types'

export interface ReviewOptions {
  state: ReviewState
  limit: number
  labels: string[]
}

const defaults: ReviewOptions = {
  state: 'open',
  limit: 20,
  labels: [],
}

const messages = {
  empty: 'No reviews yet',
  loading: 'Loading…',
  ready: 'Ready to review',
  failed: 'Could not load reviews',
  archived: 'This review is archived',
  pending: 'Waiting for a reviewer',
  approved: 'Approved',
  changes: 'Changes requested',
} as const

export async function loadReviews(path: string) {
  const source = await readFile(path, 'utf8')
  const reviews: Review[] = JSON.parse(source)
  return reviews.filter(review => review.state === defaults.state)
}

export function summarize(reviews: Review[]) {
  const counts = { open: 0, closed: 0, draft: 0 }
  for (const review of reviews) {
    counts[review.state] += 1
  }
  return counts
}

export function formatReview(review: Review) {
  const title = review.title.trim()
  const author = review.author.name
  const labels = review.labels.join(', ')
  return \`\${title} by \${author} [\${labels}]\`
}

export function sortReviews(reviews: Review[]) {
  return [...reviews].sort((a, b) => a.createdAt - b.createdAt)
}

export function groupByAuthor(reviews: Review[]) {
  const groups = new Map<string, Review[]>()
  for (const review of reviews) {
    const key = review.author.id
    const group = groups.get(key) ?? []
    group.push(review)
    groups.set(key, group)
  }
  return groups
}

export function debugReviews(reviews: Review[]) {
  console.log('Loaded reviews:', reviews.length)
  console.table(reviews)
}

export const columns = [
  { key: 'title', label: 'Title', width: 320 },
  { key: 'author', label: 'Author', width: 160 },
  { key: 'state', label: 'State', width: 100 },
  { key: 'labels', label: 'Labels', width: 240 },
] as const

export const searchPattern = /[a-z0-9_-]+/gi
export const emptyMessage = messages.empty
`,
}

const newFile = {
  name: 'src/reviews.ts',
  contents: `import { readFile } from 'node:fs/promises'
import type { Review, ReviewState } from './types'

export interface ReviewOptions {
  state: ReviewState | 'all'
  limit: number
  labels: readonly string[]
  signal?: AbortSignal
}

const defaults: ReviewOptions = {
  state: 'all',
  limit: 100,
  labels: ['bug', 'needs-review'],
}

const messages = {
  empty: 'No reviews yet',
  loading: 'Loading…',
  ready: 'Ready to review',
  failed: 'Could not load reviews',
  archived: 'This review is archived',
  pending: 'Waiting for a reviewer',
  approved: 'Approved',
  changes: 'Changes requested',
} as const

export async function loadReviews(
  path: string,
  options: Partial<ReviewOptions> = {},
): Promise<Review[]> {
  const { state, limit, labels, signal } = { ...defaults, ...options }
  const source = await readFile(path, { encoding: 'utf8', signal })
  const reviews: Review[] = JSON.parse(source)
  if (!Array.isArray(reviews)) throw new TypeError('Expected a review list')

  return reviews
    .filter(review => state === 'all' || review.state === state)
    .filter(review => labels.every(label => review.labels.includes(label)))
    .slice(0, Math.max(0, limit))
}

export function summarize(reviews: Review[]) {
  const counts = { open: 0, closed: 0, draft: 0 }
  for (const review of reviews) {
    counts[review.state] += 1
  }
  return counts
}

export function formatReview(review: Review) {
  const title = review.title.trim() || 'Untitled review'
  const author = review.author?.name ?? 'Unknown author'
  const labels = review.labels.length ? review.labels.join(' · ') : 'no labels'
  return \`\${title} — \${author} [\${labels}] — \${review.state === 'open' ? 'Ready for review ✓' : 'Archived'} — updated \${new Date(review.updatedAt).toISOString()}\`
}

export function sortReviews(reviews: Review[]) {
  return [...reviews].sort((a, b) => b.updatedAt - a.updatedAt)
}

export function groupByAuthor(reviews: Review[]) {
  const groups = new Map<string, Review[]>()
  for (const review of reviews) {
    const key = review.author.id
    const group = groups.get(key) ?? []
    group.push(review)
    groups.set(key, group)
  }
  return groups
}

export const columns = [
  { key: 'title', label: 'Title', width: 320 },
  { key: 'author', label: 'Author', width: 160 },
  { key: 'state', label: 'State', width: 100 },
  { key: 'labels', label: 'Labels', width: 240 },
] as const

export const keyboardShortcuts = new Map([
  ['j', 'Next review'],
  ['k', 'Previous review'],
  ['Enter', 'Open review'],
  ['Escape', 'Clear selection'],
])

export const searchPattern = /[\\p{L}\\p{N}_-]+/gu
export const emptyMessage = 'All caught up — 没有待审查的更改 ✓'`,
}

await preloadHighlighter({
  preferredHighlighter: 'highlights',
  themes: ['pierre-light', 'pierre-dark'],
  langs: ['typescript'],
})

const edit = document.getElementById('edit') as HTMLButtonElement
const reset = document.getElementById('reset') as HTMLButtonElement
const status = document.getElementById('status')!
const header = document.getElementById('diff-header')!
const filename = document.getElementById('filename')!
const deletions = document.getElementById('deletions')!
const additions = document.getElementById('additions')!
let editor: Editor<'file-diff'> | undefined
let finishEditing: (() => void) | undefined

const diff = new FileDiff({
  preferredHighlighter: 'highlights',
  theme: { light: 'pierre-light', dark: 'pierre-dark' },
  themeType: 'light',
  diffStyle: 'split',
  overflow: 'scroll',
  diffIndicators: 'bars',
  lineDiffType: 'word-alt',
  enableLineSelection: true,
  disableErrorHandling: true,
  renderCustomHeader(fileDiff) {
    filename.textContent = fileDiff.name
    let removed = 0
    let added = 0
    for (const hunk of fileDiff.hunks) {
      removed += hunk.deletionLines
      added += hunk.additionLines
    }
    deletions.textContent = `-${removed}`
    additions.textContent = `+${added}`
    deletions.setAttribute('aria-label', `${removed} deleted lines`)
    additions.setAttribute('aria-label', `${added} added lines`)
    return header
  },
  onEditChange({ file }) {
    reset.disabled = file.contents === newFile.contents
  },
  onEditComplete() {
    return 'accept'
  },
})

diff.render({ oldFile, newFile, fileContainer: document.getElementById('diff')! })
edit.addEventListener('click', async () => {
  edit.disabled = true
  try {
    if (finishEditing) {
      finishEditing()
      finishEditing = undefined
    } else {
      const { Editor } = await import('@pierre/diffs/edit')
      editor ??= new Editor('file-diff')
      finishEditing = editor.edit(diff)
      editor.focus()
    }
    edit.textContent = finishEditing ? 'Review' : 'Edit'
    edit.setAttribute('aria-pressed', String(!!finishEditing))
    status.textContent = finishEditing
      ? 'Edit the new file · Changes stay in this window · Use Cmd/Ctrl-Z to undo.'
      : 'Highlights · TypeScript · Select line numbers to mark a range.'
  } catch (error) {
    status.textContent = `Could not change edit mode: ${error instanceof Error ? error.message : error}`
  } finally {
    edit.disabled = false
  }
})

reset.addEventListener('click', () => {
  const editing = !!finishEditing
  editor?.cleanUp('discard')
  finishEditing = undefined
  diff.render({ oldFile, newFile, forceRender: true })
  if (editing) {
    finishEditing = editor!.edit(diff)
    editor!.focus()
  }
  reset.disabled = true
})

const layout = document.getElementById('layout') as HTMLButtonElement
layout.addEventListener('click', () => {
  const diffStyle = diff.options.diffStyle === 'split' ? 'unified' : 'split'
  diff.setOptions({ ...diff.options, diffStyle })
  diff.rerender()
  layout.textContent = diffStyle === 'split' ? 'Use unified view' : 'Use split view'
})

const theme = document.getElementById('theme') as HTMLButtonElement
theme.addEventListener('click', () => {
  const themeType = diff.options.themeType === 'light' ? 'dark' : 'light'
  diff.setThemeType(themeType)
  document.body.classList.toggle('dark', themeType === 'dark')
  theme.textContent = themeType === 'light' ? 'Use dark theme' : 'Use light theme'
})

for (const button of [edit, layout, theme]) button.disabled = false
status.textContent = 'Highlights · TypeScript · Select line numbers to mark a range.'
window.addEventListener('pagehide', () => diff.cleanUp())
