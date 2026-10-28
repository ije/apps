import { useState } from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'

function Counter() {
  const [count, setCount] = useState<number>(0)

  return (
    <main>
      <p className="eyebrow">REACT + APPS</p>
      <h1>Hello, React!</h1>
      <p className="intro">A React counter in a native window.</p>
      <p id="count" className="count">{`Count: ${count}`}</p>
      <div className="actions">
        <button id="increment" onClick={() => setCount(value => value + 1)}>Add one</button>
        <button id="reset" className="secondary" onClick={() => setCount(0)}>Reset</button>
      </div>
      {count > 0 && <p id="message">React keeps the count in state.</p>}
    </main>
  )
}

const root = createRoot(document.getElementById('root')!)
flushSync(() => root.render(<Counter />))
