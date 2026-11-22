import '@/components/ui/bar-graph'
import '@/components/ui/button'
import '@/components/ui/device'
import '@/components/ui/display'
import '@/components/ui/dpad'
import '@/components/ui/icon'
import '@/components/ui/item-list'
import '@/components/ui/slide-switch'
import { notify } from '@/components/ui/toaster'
import { escape } from '@/lib/kit'

type Deploy = { id: string; name: string; status: string; branch: string; author: string; age: string }

const deploys: Deploy[] = [
  { id: 'd1', name: 'checkout-api', status: 'Ready', branch: 'main', author: 'Ada', age: '4m' },
  { id: 'd2', name: 'web', status: 'Building', branch: 'feat/search', author: 'Grace', age: '9m' },
  { id: 'd3', name: 'billing-worker', status: 'Error', branch: 'fix/retries', author: 'Alan', age: '32m' },
  { id: 'd4', name: 'docs', status: 'Ready', branch: 'main', author: 'Katherine', age: '1h' },
  { id: 'd5', name: 'edge-config', status: 'Queued', branch: 'chore/deps', author: 'Linus', age: '2h' },
]

const logs: Record<string, string[]> = {
  'checkout-api': ['clone main @ 4f2c9e1', 'install 412 packages', 'build api', 'test 86 passed', 'upload 3 regions', 'ready in 42s'],
  web: ['clone feat/search @ 9a0be77', 'install 1,204 packages', 'build next', 'prerender 312 pages', 'optimize images 96/120'],
  'billing-worker': ['clone fix/retries @ c81d004', 'build worker', 'test 41 passed, 2 failed', 'error: timeout after 30s', 'build failed'],
  docs: ['clone main @ 77ad210', 'build content', 'index 1,840 pages', 'ready in 1m 08s'],
  'edge-config': ['queued behind 2 builds', 'waiting for a runner…'],
}

const environments = {
  Prod: { rps: 1284, errors: 0.21, p95: 182, traffic: [0.32, 0.4, 0.62, 0.78, 0.84, 0.71, 0.66, 0.74, 0.58, 0.44, 0.5, 0.61] },
  Preview: { rps: 96, errors: 1.4, p95: 240, traffic: [0.12, 0.2, 0.18, 0.3, 0.22, 0.35, 0.28, 0.4, 0.33, 0.26, 0.3, 0.24] },
  Dev: { rps: 12, errors: 3.8, p95: 410, traffic: [0.05, 0.08, 0.1, 0.06, 0.12, 0.09, 0.14, 0.1, 0.07, 0.11, 0.08, 0.13] },
}
type Env = keyof typeof environments

const pages = ['Overview', 'Deploys', 'Logs']

export default function mount(root: HTMLElement) {
  let page = 0
  let env: Env = 'Prod'
  let at = 0

  root.innerHTML = `
    <spec-device model="DX-1" name="Dashboard" on="true">
      <div data-dash>
        <div data-dash-pad>
          <spec-dpad label="Navigate"></spec-dpad>
          <small aria-hidden="true">Nav</small>
        </div>
        <div data-dash-screen>
          <spec-display aspect="16 / 10">
            <div data-dash-view aria-live="polite">
              <nav data-dash-pages aria-label="Pages">
                ${pages.map((name) => `<button type="button">${name}</button>`).join('')}
                <span></span>
              </nav>
            </div>
          </spec-display>
        </div>
        <div data-dash-right>
          <spec-slide-switch label="Environment" orientation="vertical" value="${env}">
            ${Object.keys(environments).map((name) => `<option>${name}</option>`).join('')}
          </spec-slide-switch>
          <div data-dash-ab>
            <div>
              <spec-button round aria-label="Back to overview" data-dash-back><spec-icon name="undo"></spec-icon></spec-button>
              <small aria-hidden="true">Back</small>
            </div>
            <div>
              <spec-button round tone="primary" data-dash-deploy><spec-icon name="rocket"></spec-icon></spec-button>
              <small aria-hidden="true">Deploy</small>
            </div>
          </div>
        </div>
      </div>
      <spec-item-list label="Deployments"></spec-item-list>
    </spec-device>`

  const view = root.querySelector<HTMLElement>('[data-dash-view]')!
  const tabs = [...view.querySelectorAll('button')]
  const envLabel = view.querySelector('nav > span')!
  const deployButton = root.querySelector<HTMLButtonElement>('button[data-dash-deploy]')!
  const list = root.querySelector('spec-item-list')!

  const overview = document.createElement('div')
  overview.toggleAttribute('data-dash-overview', true)
  overview.innerHTML = `<dl><div><dt>Req/s</dt><dd></dd></div><div><dt>Errors</dt><dd></dd></div><div><dt>P95</dt><dd></dd></div></dl><spec-bar-graph label="Traffic, last 24 hours"></spec-bar-graph>`
  const [rps, errors, p95] = overview.querySelectorAll('dd')
  const graph = overview.querySelector('spec-bar-graph')!

  const detail = document.createElement('div')
  detail.toggleAttribute('data-dash-detail', true)
  detail.innerHTML = `<span data-status></span><strong></strong><dl><div><dt>Branch</dt><dd></dd></div><div><dt>Author</dt><dd></dd></div><div><dt>Age</dt><dd></dd></div></dl>`
  const status = detail.querySelector('span')!
  const name = detail.querySelector('strong')!
  const [branch, author, age] = detail.querySelectorAll('dd')

  const log = document.createElement('ol')
  log.toggleAttribute('data-dash-log', true)

  const shown = [overview, detail, log]
  view.append(overview)

  const showEnv = () => {
    const data = environments[env]
    envLabel.textContent = env
    rps.textContent = data.rps.toLocaleString()
    errors.textContent = `${data.errors}%`
    p95.textContent = `${data.p95}ms`
    graph.values = data.traffic
  }

  const showDeploy = () => {
    const deploy = deploys[at]
    status.dataset.status = status.textContent = deploy.status
    name.textContent = deploy.name
    branch.textContent = deploy.branch
    author.textContent = deploy.author
    age.textContent = deploy.age
    log.innerHTML = logs[deploy.name]
      .map((line, i) => `<li${/error|failed/.test(line) ? ' data-error=""' : ''}><span>${String(i + 1).padStart(2, '0')}</span>${escape(line)}</li>`)
      .join('')
    deployButton.ariaLabel = `Redeploy ${deploy.name}`
    list.value = deploy.id
  }

  const go = (next: number) => {
    tabs[page].removeAttribute('aria-current')
    shown[page].replaceWith(shown[next])
    tabs[(page = next)].setAttribute('aria-current', 'true')
  }

  tabs[0].setAttribute('aria-current', 'true')
  tabs.forEach((tab, i) => tab.addEventListener('click', () => go(i)))

  list.columns = [
    { key: 'name', label: 'Project', width: 'minmax(0, 1.3fr)', render: (d: Deploy) => d.name },
    { key: 'branch', label: 'Branch', width: 'minmax(0, 1fr)', render: (d: Deploy) => d.branch },
    { key: 'status', label: 'Status', width: '64px', render: (d: Deploy) => d.status },
    { key: 'age', label: 'Age', width: '32px', align: 'end', render: (d: Deploy) => d.age },
  ]
  list.rowKey = (d: Deploy) => d.id
  list.rows = deploys
  list.addEventListener('valuechange', (event) => {
    at = deploys.findIndex((d) => d.id === (event as CustomEvent).detail.value)
    showDeploy()
  })

  const pad = root.querySelector('spec-dpad')!
  pad.addEventListener('left', () => go((page + pages.length - 1) % pages.length))
  pad.addEventListener('right', () => go((page + 1) % pages.length))
  pad.addEventListener('up', () => {
    at = (at + deploys.length - 1) % deploys.length
    showDeploy()
  })
  pad.addEventListener('down', () => {
    at = (at + 1) % deploys.length
    showDeploy()
  })

  root.querySelector('spec-slide-switch')!.addEventListener('valuechange', (event) => {
    env = (event as CustomEvent).detail.value
    showEnv()
  })
  root.querySelector('button[data-dash-back]')!.addEventListener('click', () => go(0))
  deployButton.addEventListener('click', () => {
    const deploy = deploys[at]
    notify(`Redeploying ${deploy.name}`, { description: `${deploy.branch} → ${env}`, duration: 2000 })
  })

  showEnv()
  showDeploy()
}
