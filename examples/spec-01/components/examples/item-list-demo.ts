import '@/components/ui/item-list'

const deploys = [
  { id: 'd41', name: 'web', branch: 'main', age: '2m', status: 'Ready' },
  { id: 'd40', name: 'api', branch: 'main', age: '18m', status: 'Ready' },
  { id: 'd39', name: 'web', branch: 'fix/nav', age: '1h', status: 'Error' },
  { id: 'd38', name: 'docs', branch: 'main', age: '3h', status: 'Ready' },
]

type Deploy = (typeof deploys)[number]

export default function mount(root: HTMLElement) {
  root.innerHTML = '<spec-item-list label="Deployments"></spec-item-list>'
  const list = root.querySelector('spec-item-list')!
  list.rowKey = (row: Deploy) => row.id
  list.value = 'd40'
  list.columns = [
    { key: 'id', label: 'Id', width: '34px', render: (row: Deploy) => row.id },
    { key: 'name', label: 'Project', width: 'minmax(0, 1fr)', render: (row: Deploy) => row.name },
    { key: 'branch', label: 'Branch', width: 'minmax(0, 1fr)', render: (row: Deploy) => row.branch },
    { key: 'status', label: 'Status', width: '48px', render: (row: Deploy) => row.status },
    { key: 'age', label: 'Age', width: '30px', align: 'end', render: (row: Deploy) => row.age },
  ]
  list.rows = deploys
}
