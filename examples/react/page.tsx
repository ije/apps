import { useState } from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'

function App() {
  const [count, setCount] = useState<number>(0)
  const size = 4 + Math.min(count, 6) * 2

  return (
    <main className="mx-auto flex max-w-[760px] flex-col gap-6 p-6 md:p-12">
      <p className="text-xs font-bold">REACT + TAILWIND</p>
      <h1 className="text-3xl font-bold md:text-5xl">Hello, React!</h1>
      <p className="text-lg text-muted">React state and Tailwind classes in a native window. No Tailwind build step.</p>
      <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
        <section className="card">
          <h2 className="text-lg font-semibold">State</h2>
          <p id="count" className="text-3xl font-bold">{`Count: ${count}`}</p>
          {count > 0 && <p id="message" className="text-sm text-muted">React keeps the count in state.</p>}
        </section>
        <section className="card">
          <h2 className="text-lg font-semibold">Classes can change</h2>
          <div className="flex h-16 items-center gap-3">
            <div className={`size-${size} rounded-full bg-mint-500`} />
            <span className="text-sm">{`size-${size}`}</span>
          </div>
        </section>
      </div>
      <div className="flex gap-3">
        <button id="increment" className="cursor-pointer rounded-lg bg-mint-500 px-5 py-3 font-semibold" onClick={() => setCount(value => value + 1)}>Add one</button>
        <button id="reset" className="cursor-pointer rounded-lg bg-white px-5 py-3 font-semibold" onClick={() => setCount(0)}>Reset</button>
      </div>
      <p className="text-sm text-muted">Resize the window to switch between one and two columns.</p>
    </main>
  )
}

const root = createRoot(document.getElementById('root')!)
flushSync(() => root.render(<App />))
